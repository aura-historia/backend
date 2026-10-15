use application::operation_context::{CredentialCapability, OperationContext, Principal};
use aura_historia_api::{
    app,
    auth::{AuthError, AuthMethod, RequestMetadata, TokenAuthenticator, TransportPrincipal},
    state::{AppState, ProductListingsState},
};
use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode},
};
use geo::{AddressText, CountryCode, GeographicDescription};
use listing_source_core::ListingSourceId;
use listing_source_postgres::SqlxSourceLocationAccessFactory;
use party_core::party_id::PartyId;
use platform_postgres::SqlxUnitOfWork;
use product_listing_core::product_listing_id::ProductListingId;
use product_listing_service::{shipping::ShippingEnrichment, use_cases::*};
use serde_json::{Value, json};
use shipping_core::{fixtures::gallery_terms, profile::*};
use shipping_postgres::{SqlxShippingRepositoryFactory, assessment::SqlxAssessmentReaderFactory};
use shipping_service::{assessment::*, ports::ShippingDefaultRevision, use_cases::*};
use sqlx::PgPool;
use std::sync::Arc;
use test_api::{IntegrationTestService, Postgres, aura_integration_test, get_postgres_client};
use tower::ServiceExt;
use user_core::{user_address::*, user_id::UserId};
use user_service::address_book::*;
const SCHEMA: Postgres = Postgres::new("migrations");
type Book = AddressBookHandler<
    SqlxUnitOfWork,
    user_postgres::SqlxAddressBookFactory,
    geo_postgres::SqlxResolutionRepositoryFactory,
>;
struct Auth(UserId, UserId);
#[async_trait::async_trait]
impl TokenAuthenticator for Auth {
    async fn authenticate(
        &self,
        token: &str,
        _: &RequestMetadata,
    ) -> Result<TransportPrincipal, AuthError> {
        let (user, method, capabilities) = match token {
            "owner" => (self.0, AuthMethod::CognitoJwt, Default::default()),
            "other" => (self.1, AuthMethod::CognitoJwt, Default::default()),
            "delegated" => (
                self.0,
                AuthMethod::AuraAccessToken,
                [CredentialCapability::UsersRead].into(),
            ),
            "no-user-scope" => (
                self.0,
                AuthMethod::AuraAccessToken,
                [CredentialCapability::ProductListingsRead].into(),
            ),
            _ => return Err(AuthError::InvalidCredentials),
        };
        Ok(TransportPrincipal::User {
            user_id: user,
            auth_method: method,
            capabilities,
        })
    }
}
fn ctx(principal: Principal) -> OperationContext {
    OperationContext {
        principal,
        request_id: "shipping-api-test".into(),
        correlation_id: "shipping-api-test".into(),
    }
}
async fn fixture() -> (
    PgPool,
    axum::Router,
    Arc<Book>,
    UserId,
    ShippingProfileId,
    ProductListingId,
) {
    let p = get_postgres_client().await;
    sqlx::query("UPDATE geo_resolution_provider_controls SET is_default=false")
        .execute(&p)
        .await
        .unwrap();
    sqlx::query("INSERT INTO geo_resolution_provider_controls(profile,is_default) VALUES('SHIPPING_DESTINATION_TEST',true)").execute(&p).await.unwrap();
    let owner = UserId::new();
    let other = UserId::new();
    let party = PartyId::new();
    let source = ListingSourceId::new();
    for id in [owner, other] {
        sqlx::query("INSERT INTO users(user_id,email,tier,role)VALUES($1,$2,'FREE','USER')")
            .bind(id.into_uuid())
            .bind(format!("{id}@example.test"))
            .execute(&p)
            .await
            .unwrap();
    }
    sqlx::query("INSERT INTO parties(party_id,party_slug_id,name)VALUES($1,$2,'Gallery')")
        .bind(party.into_uuid())
        .bind(format!("gallery-{}", party.as_uuid().simple()))
        .execute(&p)
        .await
        .unwrap();
    sqlx::query("INSERT INTO listing_sources(listing_source_id,listing_source_slug_id,name,operator_party_id)VALUES($1,$2,'Gallery',$3)").bind(source.into_uuid()).bind(format!("source-{}",source.as_uuid().simple())).bind(party.into_uuid()).execute(&p).await.unwrap();
    let uow = SqlxUnitOfWork::new(p.clone());
    let profile = CreateShippingProfileHandler::new(
        uow.clone(),
        SqlxShippingRepositoryFactory,
        SqlxSourceLocationAccessFactory,
    )
    .execute(
        &ctx(Principal::System),
        CreateProfileCommand {
            source_id: source,
            creation_key: "gallery".into(),
            content: ShippingProfileContent {
                name: ShippingProfileName::new("Gallery terms".into()).unwrap(),
                attributed_party: None,
                terms: gallery_terms(8000),
            },
            evidence: vec![
                shipping_service::ports::ShippingEvidence::new(
                    "https://example.test/protected-evidence".into(),
                    "MANUAL_ASSERTION".into(),
                )
                .unwrap(),
            ],
        },
    )
    .await
    .unwrap();
    AssignShippingDefaultHandler::new(
        uow.clone(),
        SqlxShippingRepositoryFactory,
        SqlxSourceLocationAccessFactory,
    )
    .execute(
        &ctx(Principal::System),
        AssignDefaultCommand {
            source_id: source,
            profile_id: profile.id,
            expected_revision: ShippingDefaultRevision::INITIAL,
        },
    )
    .await
    .unwrap();
    let id = ProductListingId::new();
    let event = domain_primitives::event_id::EventId::new().into_uuid();
    let mut listing_tx = p.begin().await.unwrap();
    sqlx::query("INSERT INTO product_listings(product_listing_id,product_listing_title_slug_id,current_event_id,content_source_event_id,embedding_source_event_id,listing_source_id,source_listing_id,lifecycle,url)VALUES($1,$2,$3,$3,$3,$4,$2,'ACTIVE','https://example.test/item')").bind(id.into_uuid()).bind(format!("item-{}-abcdef",id.as_uuid().simple())).bind(event).bind(source.into_uuid()).execute(&mut *listing_tx).await.unwrap();
    sqlx::query("INSERT INTO product_listing_events(event_id,product_listing_id,event_type,event_group,event_type_schema_version,payload,event_time)VALUES($1,$2,'PRODUCT_LISTING_DISCOVERED','DOMAIN',1,$3,now())").bind(event).bind(id.into_uuid()).bind(serde_json::json!({"listingSourceId":source.into_uuid().to_string(),"sourceListingId":format!("item-{}-abcdef",id.as_uuid().simple()),"title":null,"description":null,"pricing":{"price":null,"priceEstimateMin":null,"priceEstimateMax":null},"availability":null,"url":"https://example.test/item","imageCount":0,"auction":null})).execute(&mut *listing_tx).await.unwrap();
    listing_tx.commit().await.unwrap();
    let book = Arc::new(AddressBookHandler::new(
        uow.clone(),
        user_postgres::SqlxAddressBookFactory,
        geo_postgres::SqlxResolutionRepositoryFactory,
    ));
    let fx = fxrate_core::FxRateId::new().into_uuid();
    sqlx::query("INSERT INTO fx_rates(fx_rate_id,captured_at,source,source_event_id)VALUES($1,now(),'fxratesapi',$2)")
        .bind(fx).bind(fx.to_string()).execute(&p).await.unwrap();
    for currency in [
        "EUR", "GBP", "USD", "AUD", "CAD", "NZD", "CNY", "BRL", "PLN", "TRY", "JPY", "CZK", "RUB",
        "AED", "SAR", "HKD", "SGD", "CHF", "ZAR", "SEK", "DKK", "NOK", "KRW", "INR", "TWD", "HUF",
        "RON", "MXN", "THB",
    ] {
        sqlx::query(
            "INSERT INTO fx_rate_quotes(fx_rate_id,currency,units_per_eur)VALUES($1,$2,1000000)",
        )
        .bind(fx)
        .bind(currency)
        .execute(&p)
        .await
        .unwrap();
    }
    let shipping = ShippingEnrichment::new(Arc::new(AssessDeliveryHandler::new(
        uow.clone(),
        SqlxAssessmentReaderFactory,
        book.clone(),
    )));
    let router = app(AppState::new().with_products(ProductListingsState::new(
        Arc::new(GetProductListingHandler::new(
            uow,
            product_listing_postgres::SqlxProductListingDetailsReaderFactory::new(),
            fxrate_postgres::SqlxFxRateSnapshotRepositoryFactory,
            shipping,
        )),
        Arc::new(UnusedQueries),
        Arc::new(UnusedQueries),
        Arc::new(Auth(owner, other)),
    )));
    (p, router, book, owner, profile.id, id)
}
struct UnusedQueries;
#[async_trait::async_trait]
impl GetSimilarProductListingsUseCase for UnusedQueries {
    async fn execute(
        &self,
        _: &OperationContext,
        _: GetSimilarProductListingsRequest,
    ) -> Result<GetSimilarProductListingsResult, GetSimilarProductListingsError> {
        unreachable!()
    }
}
#[async_trait::async_trait]
impl SearchProductListingsUseCase for UnusedQueries {
    async fn execute(
        &self,
        _: &OperationContext,
        _: SearchProductListingsRequest,
    ) -> Result<SearchProductListingsResult, SearchProductListingsError> {
        unreachable!()
    }
}
async fn request(router: &axum::Router, path: &str, token: Option<&str>) -> (StatusCode, Value) {
    let mut req = Request::get(path);
    if let Some(token) = token {
        req = req.header("authorization", format!("Bearer {token}"));
    }
    let res = router
        .clone()
        .oneshot(req.body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = res.status();
    if token.is_some() || status != StatusCode::OK {
        assert_eq!("private, no-store", res.headers()["cache-control"]);
    } else {
        assert!(
            res.headers()["cache-control"]
                .to_str()
                .unwrap()
                .starts_with("public, max-age=0, s-maxage=")
        );
    }
    let bytes = to_bytes(res.into_body(), 1_000_000).await.unwrap();
    let text = std::str::from_utf8(&bytes).unwrap();
    for private in [
        "private saved street",
        "protected-evidence",
        "MANUAL_ASSERTION",
        "address_text",
        "private origin street",
        "private pickup street",
    ] {
        assert!(
            !text.contains(private),
            "private information leaked: {private}"
        );
    }
    (status, serde_json::from_slice(&bytes).unwrap())
}
async fn select_country(book: &Book, owner: UserId, postal: Option<&str>, version: u64) {
    book.set_destination(
        &ctx(Principal::User(owner)),
        BrowsingDestination::CountryPostal {
            country: CountryCode::DEU,
            postal_code: postal.map(|v| geo::PostalCode::new(v).unwrap()),
        },
        version,
    )
    .await
    .unwrap();
}
#[aura_integration_test(services=[SCHEMA])]
async fn listings_return_generic_terms_and_selected_destination_personalization() {
    let (_, router, book, owner, _, id) = fixture().await;
    let path = format!("/api/v1/product-listings/{id}");
    let (status, anon) = request(&router, &path, None).await;
    assert_eq!(StatusCode::OK, status, "{anon}");
    let shipping = &anon["item"]["shipping"];
    assert_eq!(
        8000,
        shipping["terms"]["shipping"]["coverage"]["zones"][0]["services"][0]["rate"]["price"]["amount"]
            ["amount_minor"]
    );
    assert!(anon.get("userState").is_none());
    let (_, no_destination) = request(&router, &path, Some("owner")).await;
    assert!(no_destination["userState"].get("shipping").is_none());
    assert_eq!(
        shipping["terms"],
        no_destination["item"]["shipping"]["terms"]
    );
    // Saving an address does not select it for browsing.
    let address = book
        .create(
            &ctx(Principal::User(owner)),
            CreateUserAddress {
                idempotency_key: UserAddressIdempotencyKey::new("private".into()).unwrap(),
                label: UserAddressLabel::new("Destination".into()).unwrap(),
                geography: GeographicDescription::new(
                    Some(AddressText::new("private saved street").unwrap()),
                    Some(CountryCode::DEU),
                    None,
                    None,
                )
                .unwrap()
                .unwrap(),
            },
        )
        .await
        .unwrap();
    let (_, unselected) = request(&router, &path, Some("owner")).await;
    assert!(unselected["userState"].get("shipping").is_none());
    book.set_destination(
        &ctx(Principal::User(owner)),
        BrowsingDestination::Address(address.id),
        1,
    )
    .await
    .unwrap();
    let (_, selected) = request(&router, &path, Some("owner")).await;
    let personal = &selected["userState"]["shipping"];
    assert_eq!("OFFERED", personal["eligibility"]);
    assert_eq!("DE", personal["destination"]["country"]);
    assert_eq!(0, personal["selectedZone"]);
    assert_eq!(0, personal["options"][0]["serviceIndex"]);
    assert!(personal["options"][0]["charge"].is_null());
    assert!(
        personal["options"][0]["reasons"]
            .as_array()
            .unwrap()
            .contains(&json!({"code":"MISSING_INPUT","input":"QUANTITY"}))
    );
    assert_eq!(false, personal["bindingQuote"]);
    assert_eq!(false, personal["completeLandedCost"]);
    assert!(!selected.to_string().contains(&address.id.to_string()));
    let (_, other) = request(&router, &path, Some("other")).await;
    assert!(other["userState"].get("shipping").is_none());
    let (_, delegated) = request(&router, &path, Some("delegated")).await;
    assert_eq!("OFFERED", delegated["userState"]["shipping"]["eligibility"]);
    let (_, scoped) = request(&router, &path, Some("no-user-scope")).await;
    assert!(scoped["userState"].get("shipping").is_none());
    let slug = selected["item"]["productListingTitleSlugId"]
        .as_str()
        .unwrap();
    let (status, by_slug) = request(
        &router,
        &format!("/api/v1/product-listings/by-slug/{slug}"),
        Some("owner"),
    )
    .await;
    assert_eq!(StatusCode::OK, status);
    assert_eq!("OFFERED", by_slug["userState"]["shipping"]["eligibility"]);
    book.delete(&ctx(Principal::User(owner)), address.id, 1)
        .await
        .unwrap();
    let (_, deleted) = request(&router, &path, Some("owner")).await;
    assert!(deleted["userState"].get("shipping").is_none());
    assert_eq!(
        StatusCode::UNAUTHORIZED,
        request(&router, &path, Some("invalid")).await.0
    );
}
#[aura_integration_test(services=[SCHEMA])]
async fn current_profile_changes_overrides_expiry_and_suspension_are_reflected() {
    let (p, router, book, owner, profile, id) = fixture().await;
    select_country(&book, owner, None, 1).await;
    let path = format!("/api/v1/product-listings/{id}");
    let (_, first) = request(&router, &path, Some("owner")).await;
    let source: ListingSourceId = first["item"]["source"]["listingSourceId"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();
    ReplaceShippingProfileHandler::new(
        SqlxUnitOfWork::new(p.clone()),
        SqlxShippingRepositoryFactory,
        SqlxSourceLocationAccessFactory,
    )
    .execute(
        &ctx(Principal::System),
        ReplaceProfileCommand {
            source_id: source,
            profile_id: profile,
            expected_revision: shipping_service::ports::ShippingManagementRevision::INITIAL,
            content: ShippingProfileContent {
                name: ShippingProfileName::new("Gallery terms".into()).unwrap(),
                attributed_party: None,
                terms: gallery_terms(9500),
            },
            evidence: vec![],
        },
    )
    .await
    .unwrap();
    let (_, updated) = request(&router, &path, Some("owner")).await;
    assert_eq!(2, updated["item"]["shipping"]["profile"]["termsRevision"]);
    assert_eq!(
        9500,
        updated["item"]["shipping"]["terms"]["shipping"]["coverage"]["zones"][0]["services"][0]["rate"]
            ["price"]["amount"]["amount_minor"]
    );
    let cabinet = json!({"schema":"LISTING_DELIVERY_V1","source_id":source.into_uuid().to_string(),"integration":{"kind":"SOURCE"},"selection":{"kind":"INHERIT_SOURCE_DEFAULT"},"rate_inputs":{"classes":null,"variants":null},"shipping":{"kind":"REPLACE","value":{"kind":"PROHIBITED"}},"collection":{"kind":"INHERIT"}});
    sqlx::query(
        "UPDATE product_listings SET delivery_configuration=$2 WHERE product_listing_id=$1",
    )
    .bind(id.into_uuid())
    .bind(cabinet)
    .execute(&p)
    .await
    .unwrap();
    let (_, cabinet) = request(&router, &path, Some("owner")).await;
    assert_eq!(
        "NOT_OFFERED",
        cabinet["userState"]["shipping"]["eligibility"]
    );
    assert_eq!(
        "OFFERED",
        cabinet["userState"]["shipping"]["collection"]["eligibility"]
    );
    sqlx::query(
        "UPDATE product_listings SET delivery_configuration=NULL WHERE product_listing_id=$1",
    )
    .bind(id.into_uuid())
    .execute(&p)
    .await
    .unwrap();
    sqlx::query("UPDATE shipping_profiles SET fresh_until=now()+interval '30 seconds' WHERE shipping_profile_id=$1")
        .bind(profile.into_uuid()).execute(&p).await.unwrap();
    let anonymous = router
        .clone()
        .oneshot(Request::get(&path).body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(StatusCode::OK, anonymous.status());
    let cache = anonymous.headers()["cache-control"].to_str().unwrap();
    let ttl: u32 = cache
        .split("s-maxage=")
        .nth(1)
        .unwrap()
        .split(',')
        .next()
        .unwrap()
        .parse()
        .unwrap();
    assert!(
        ttl <= 30,
        "shipping evidence expires before the normal 120s detail TTL: {cache}"
    );
    sqlx::query("UPDATE shipping_profiles SET fresh_until=verified_at,expires_at=now()+interval '1 hour' WHERE shipping_profile_id=$1").bind(profile.into_uuid()).execute(&p).await.unwrap();
    let (_, stale) = request(&router, &path, Some("owner")).await;
    assert_eq!(
        "STALE",
        stale["item"]["shipping"]["profile"]["evidence"]["status"]
    );
    assert_eq!(
        updated["item"]["shipping"]["profile"]["evidence"]["verifiedAt"],
        stale["item"]["shipping"]["profile"]["evidence"]["verifiedAt"]
    );
    sqlx::query("UPDATE shipping_profiles SET expires_at=fresh_until WHERE shipping_profile_id=$1")
        .bind(profile.into_uuid())
        .execute(&p)
        .await
        .unwrap();
    let (_, expired) = request(&router, &path, Some("owner")).await;
    assert!(expired["userState"]["shipping"]["eligibility"].is_null());
    assert!(expired["item"]["shipping"]["terms"]["shipping"].is_null());
    assert_eq!(
        "EXPIRED",
        expired["item"]["shipping"]["profile"]["evidence"]["status"]
    );
    sqlx::query("UPDATE users SET suspended=true WHERE user_id=$1")
        .bind(owner.into_uuid())
        .execute(&p)
        .await
        .unwrap();
    assert_eq!(
        StatusCode::FORBIDDEN,
        request(&router, &path, Some("owner")).await.0
    );
    assert_eq!(StatusCode::OK, request(&router, &path, None).await.0);
    for path in [
        format!("/api/v1/product-listings/{id}/delivery-assessment"),
        "/api/v1/product-listings/delivery-assessments".into(),
    ] {
        let response = router
            .clone()
            .oneshot(Request::post(path).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert!(matches!(
            response.status(),
            StatusCode::NOT_FOUND | StatusCode::METHOD_NOT_ALLOWED
        ));
    }
}
#[aura_integration_test(services=[SCHEMA])]
async fn current_location_eligibility_and_postal_exclusions_preserve_collection_privacy() {
    use shipping_core::{
        coverage::*, inputs::Applicability, terms::*, time::CollectionArrangement,
    };
    let (p, router, book, user, profile, id) = fixture().await;
    select_country(&book, user, None, 1).await;
    let path = format!("/api/v1/product-listings/{id}");
    let(source,owner):(uuid::Uuid,uuid::Uuid)=sqlx::query_as("SELECT s.listing_source_id,s.operator_party_id FROM listing_sources s JOIN product_listings l USING(listing_source_id) WHERE l.product_listing_id=$1").bind(id.into_uuid()).fetch_one(&p).await.unwrap();
    let source = ListingSourceId::try_from(source).unwrap();
    let site = party_core::party_location_id::PartyLocationId::new();
    sqlx::query("INSERT INTO party_locations(party_location_id,party_id,label,geography,lifecycle,created_actor,updated_actor)VALUES($1,$2,'Dispatch',$3,'ACTIVE','test','test')").bind(site.into_uuid()).bind(owner).bind(json!({"reference_release":"iso-codes-4.20.1","address_text":"private origin street","country":"DE","subdivision":null,"postal_code":null})).execute(&p).await.unwrap();
    let base = gallery_terms(8000);
    let c = match base.shipping().unwrap() {
        ShippingPolicy::Coverage(c) => c,
        _ => unreachable!(),
    };
    let collection = CollectionPolicy::new(
        Applicability::default(),
        CollectionDependency::Independent,
        Some(ShippingEligibility::Offered),
        Some(DeliverySite::Asserted(
            SiteAssertions::new(
                GeographicDescription::new(
                    Some(AddressText::new("private pickup street").unwrap()),
                    Some(CountryCode::DEU),
                    None,
                    None,
                )
                .unwrap(),
                None,
            )
            .unwrap(),
        )),
        Some(CollectionArrangement::ByAppointment),
    );
    let terms = DeliveryTerms::new(
        Some(ShippingPolicy::coverage(
            ShippingCoverage::new(
                c.release(),
                c.completeness(),
                c.applicability().clone(),
                vec![],
                c.zones().to_vec(),
                Some(DeliverySite::PartyLocation(site)),
            )
            .unwrap(),
        )),
        Some(collection.clone()),
    );
    let replace = ReplaceShippingProfileHandler::new(
        SqlxUnitOfWork::new(p.clone()),
        SqlxShippingRepositoryFactory,
        SqlxSourceLocationAccessFactory,
    );
    let changed = replace
        .execute(
            &ctx(Principal::System),
            ReplaceProfileCommand {
                source_id: source,
                profile_id: profile,
                expected_revision: shipping_service::ports::ShippingManagementRevision::INITIAL,
                content: ShippingProfileContent {
                    name: ShippingProfileName::new("Terms".into()).unwrap(),
                    attributed_party: None,
                    terms,
                },
                evidence: vec![],
            },
        )
        .await
        .unwrap();
    let (_, active) = request(&router, &path, Some("owner")).await;
    assert_eq!("OFFERED", active["userState"]["shipping"]["eligibility"]);
    assert!(
        active["item"]["shipping"]["terms"]["shipping"]["coverage"]["dispatch_origin"].is_null()
    );
    assert!(active["item"]["shipping"]["terms"]["collection"]["site"].is_null());
    assert!(!active.to_string().contains("private pickup street"));
    assert!(!active.to_string().contains("private origin street"));
    sqlx::query("UPDATE party_locations SET lifecycle='RETIRED',revision=revision+1 WHERE party_location_id=$1").bind(site.into_uuid()).execute(&p).await.unwrap();
    let (_, retired) = request(&router, &path, Some("owner")).await;
    assert!(retired["userState"]["shipping"]["eligibility"].is_null());
    assert_eq!(
        json!([{"code":"LOCATION_RETIRED"}]),
        retired["userState"]["shipping"]["reasons"]
    );
    assert_eq!(
        "OFFERED",
        retired["userState"]["shipping"]["collection"]["eligibility"]
    );
    // Remove the retired origin explicitly, retaining a country rule with a postal exclusion.
    let exclusion = DeliveryArea::Postal(
        PostalRule::new(
            PostalRuleVersion::V1,
            ReferenceCountry::new("DE", c.release()).unwrap(),
            PostalForm::GermanPrefix("1".into()),
        )
        .unwrap(),
    );
    let terms = DeliveryTerms::new(
        Some(ShippingPolicy::coverage(
            ShippingCoverage::new(
                c.release(),
                c.completeness(),
                c.applicability().clone(),
                vec![exclusion],
                c.zones().to_vec(),
                None,
            )
            .unwrap(),
        )),
        Some(collection),
    );
    replace
        .execute(
            &ctx(Principal::System),
            ReplaceProfileCommand {
                source_id: source,
                profile_id: profile,
                expected_revision: changed.management_revision,
                content: ShippingProfileContent {
                    name: ShippingProfileName::new("Terms".into()).unwrap(),
                    attributed_party: None,
                    terms,
                },
                evidence: vec![],
            },
        )
        .await
        .unwrap();
    let (_, unknown) = request(&router, &path, Some("owner")).await;
    assert!(unknown["userState"]["shipping"]["eligibility"].is_null());
    assert_eq!(json!([]), unknown["userState"]["shipping"]["options"]);
    assert!(
        unknown["userState"]["shipping"]["reasons"]
            .as_array()
            .unwrap()
            .contains(&json!({"code":"MISSING_INPUT","input":"POSTAL_CODE"}))
    );
    select_country(&book, user, Some("20000"), 2).await;
    let (_, offered) = request(&router, &path, Some("owner")).await;
    assert_eq!("OFFERED", offered["userState"]["shipping"]["eligibility"]);
}

#[aura_integration_test(services=[SCHEMA])]
async fn synchronized_gallery_terms_are_visible_without_a_projection_wait() {
    use shipping_sync_service::{use_cases::*, *};
    let (p, router, book, owner, _, id) = fixture().await;
    let (source, source_key): (uuid::Uuid, String) = sqlx::query_as(
        "SELECT listing_source_id,source_listing_id FROM product_listings WHERE product_listing_id=$1",
    )
    .bind(id.into_uuid())
    .fetch_one(&p)
    .await
    .unwrap();
    let source = ListingSourceId::try_from(source).unwrap();
    let key = |value: &str| ExternalKey::new(value.into()).unwrap();
    let sync = ShippingSyncHandler::new(
        SqlxUnitOfWork::new(p.clone()),
        shipping_sync_postgres::SqlxShippingSyncStoreFactory,
        SqlxSourceLocationAccessFactory,
        canonical::CanonicalShippingSync::new(
            shipping_sync_postgres::SqlxShippingSyncStoreFactory,
            SqlxShippingRepositoryFactory,
            product_listing_postgres::SqlxProductListingRepositoryFactory,
            product_listing_postgres::SqlxProductListingEventAppenderFactory,
            product_listing_postgres::delivery::SqlxProductListingDeliveryReferencesFactory,
        ),
    );
    let context = ctx(Principal::System);
    let SyncResult::Binding(binding) = sync
        .execute(
            &context,
            source,
            SyncCommand::Rotate {
                integration: product_listing_core::delivery::DeliveryIntegration::Source,
                expected_revision: 0,
            },
        )
        .await
        .unwrap()
    else {
        panic!("installation binding");
    };
    select_country(&book, owner, None, 1).await;
    for amount in [8000, 9500] {
        let SyncResult::Head(head) = sync
            .execute(
                &context,
                source,
                SyncCommand::Head {
                    epoch: binding.epoch,
                    scope: key("gallery"),
                },
            )
            .await
            .unwrap()
        else {
            panic!("scope head");
        };
        let SyncResult::Receipt(receipt) = sync
            .execute(
                &context,
                source,
                SyncCommand::Begin(Begin {
                    epoch: binding.epoch,
                    key: key(&format!("gallery-{amount}")),
                    scope: key("gallery"),
                    mode: Mode::Snapshot,
                    expected_generation: head.generation,
                    expected_sequence: head.sequence,
                    provider_version: key("website-v1"),
                    components: [true, false, true],
                }),
            )
            .await
            .unwrap()
        else {
            panic!("fetch receipt");
        };
        let now = time::OffsetDateTime::now_utc();
        let window = ObservationWindow {
            observed_at: now,
            fresh_until: now + time::Duration::hours(1),
            expires_at: now + time::Duration::hours(2),
        };
        let records = vec![
            Record {
                key: key("policy"),
                window,
                objects: vec![],
                assertion: Assertion::Profile {
                    content: Box::new(ShippingProfileContent {
                        name: ShippingProfileName::new("Acquired gallery terms".into()).unwrap(),
                        attributed_party: None,
                        terms: gallery_terms(amount),
                    }),
                    evidence: vec![],
                },
            },
            Record {
                key: key(&source_key),
                window,
                objects: vec![],
                assertion: Assertion::Item {
                    source_listing_id: source_key.clone().try_into().unwrap(),
                    assertion: Box::new(ItemAssertion {
                        selection: EvidenceSelection::Profile(key("policy")),
                        rate_inputs: Default::default(),
                        overrides: Default::default(),
                    }),
                },
            },
        ];
        let chunk = shipping_sync_json::decode_chunk::<shipping_sync_json::PublicReferences>(
            shipping_sync_json::encode_chunk::<shipping_sync_json::PublicReferences>(&records)
                .unwrap(),
            source,
        )
        .unwrap();
        sync.execute(
            &context,
            source,
            SyncCommand::Capture {
                id: receipt.id,
                index: 0,
                chunk: chunk.clone(),
            },
        )
        .await
        .unwrap();
        let SyncResult::Receipt(applied) = sync
            .execute(
                &context,
                source,
                SyncCommand::Finalize {
                    id: receipt.id,
                    manifest: shipping_sync_json::manifest(&[chunk]),
                },
            )
            .await
            .unwrap()
        else {
            panic!("application receipt");
        };
        assert_eq!(ApplicationState::Applied, applied.state);
        let path = format!("/api/v1/product-listings/{id}");
        let (status, public) = request(&router, &path, None).await;
        assert_eq!(StatusCode::OK, status, "{public}");
        assert_eq!(
            amount,
            public["item"]["shipping"]["terms"]["shipping"]["coverage"]["zones"][0]["services"][0]
                ["rate"]["price"]["amount"]["amount_minor"]
        );
        let (status, personalized) = request(&router, &path, Some("owner")).await;
        assert_eq!(StatusCode::OK, status, "{personalized}");
        assert_eq!(
            public["item"]["shipping"]["terms"],
            personalized["item"]["shipping"]["terms"]
        );
        assert_eq!(
            "OFFERED",
            personalized["userState"]["shipping"]["eligibility"]
        );
        assert_eq!(false, personalized["userState"]["shipping"]["bindingQuote"]);
    }
}
