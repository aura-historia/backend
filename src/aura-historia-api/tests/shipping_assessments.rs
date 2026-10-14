use application::operation_context::{CredentialCapability, OperationContext, Principal};
use aura_historia_api::{
    app,
    auth::{AuthError, AuthMethod, RequestMetadata, TokenAuthenticator, TransportPrincipal},
    shipping::assessment::AssessmentState,
    state::AppState,
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
    let event = uuid::Uuid::new_v4();
    let mut listing_tx = p.begin().await.unwrap();
    sqlx::query("INSERT INTO product_listings(product_listing_id,product_listing_title_slug_id,current_event_id,content_source_event_id,embedding_source_event_id,listing_source_id,source_listing_id,lifecycle,url)VALUES($1,$2,$3,$3,$3,$4,$2,'ACTIVE','https://example.test/item')").bind(id.into_uuid()).bind(format!("item-{}-abcdef",id.as_uuid().simple())).bind(event).bind(source.into_uuid()).execute(&mut *listing_tx).await.unwrap();
    sqlx::query("INSERT INTO product_listing_events(event_id,product_listing_id,event_type,event_group,event_type_schema_version,payload,event_time)VALUES($1,$2,'PRODUCT_LISTING_DISCOVERED','DOMAIN',1,$3,now())").bind(event).bind(id.into_uuid()).bind(serde_json::json!({"listingSourceId":source.into_uuid().to_string(),"sourceListingId":format!("item-{}-abcdef",id.as_uuid().simple()),"title":null,"description":null,"pricing":{"price":null,"priceEstimateMin":null,"priceEstimateMax":null},"availability":null,"url":"https://example.test/item","imageCount":0,"auction":null})).execute(&mut *listing_tx).await.unwrap();
    listing_tx.commit().await.unwrap();
    let book = Arc::new(AddressBookHandler::new(
        uow.clone(),
        user_postgres::SqlxAddressBookFactory,
        geo_postgres::SqlxResolutionRepositoryFactory,
    ));
    let router = app(AppState::new().with_shipping_assessments(AssessmentState {
        assess: Arc::new(AssessDeliveryHandler::new(
            uow,
            SqlxAssessmentReaderFactory,
            book.clone(),
        )),
        authenticator: Arc::new(Auth(owner, other)),
    }));
    (p, router, book, owner, profile.id, id)
}
async fn request(
    router: &axum::Router,
    path: &str,
    token: Option<&str>,
    body: Value,
) -> (StatusCode, Value) {
    let mut req = Request::post(path).header("content-type", "application/json");
    if let Some(token) = token {
        req = req.header("authorization", format!("Bearer {token}"));
    }
    let res = router
        .clone()
        .oneshot(req.body(Body::from(body.to_string())).unwrap())
        .await
        .unwrap();
    assert_eq!("private, no-store", res.headers()["cache-control"]);
    let status = res.status();
    let bytes = to_bytes(res.into_body(), 1_000_000).await.unwrap();
    let text = std::str::from_utf8(&bytes).unwrap();
    for private in [
        "private saved street",
        "protected-evidence",
        "MANUAL_ASSERTION",
        "address_text",
        "latitude_degrees",
    ] {
        assert!(!text.contains(private), "private content leaked: {private}");
    }
    (status, serde_json::from_slice(&bytes).unwrap())
}
fn explicit() -> Value {
    json!({"destination":{"kind":"GEOGRAPHIC","country":"DE"},"input":{"quantity":1}})
}
#[aura_integration_test(services=[SCHEMA])]
async fn current_api_profile_updates_freshness_prices_and_collection() {
    let (p, router, _, _, profile, id) = fixture().await;
    let path = format!("/api/v1/product-listings/{id}/delivery-assessment");
    let (status, first) = request(&router, &path, None, explicit()).await;
    assert_eq!(StatusCode::OK, status);
    assert_eq!("OFFERED", first["items"][0]["eligibility"]);
    assert_eq!(
        8000,
        first["items"][0]["options"][0]["charge"]["amount"]["amount_minor"]
    );
    assert_eq!(
        "EUR",
        first["items"][0]["options"][0]["charge"]["amount"]["currency"]
    );
    assert_eq!(
        "PER_ITEM",
        first["items"][0]["options"][0]["service"]["rate"]["basis"]
    );
    assert_eq!(
        "BY_APPOINTMENT",
        first["items"][0]["terms"]["collection"]["arrangement"]["kind"]
    );
    assert_eq!(false, first["items"][0]["bindingQuote"]);
    assert_eq!(false, first["items"][0]["completeLandedCost"]);
    let source: ListingSourceId = first["items"][0]["listingSourceId"]
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
    let (_, updated) = request(&router, &path, None, explicit()).await;
    assert_eq!(
        9500,
        updated["items"][0]["options"][0]["charge"]["amount"]["amount_minor"]
    );
    assert_eq!(2, updated["items"][0]["profile"]["termsRevision"]);
    let (_, no_quantity) = request(
        &router,
        &path,
        None,
        json!({"destination":{"kind":"GEOGRAPHIC","country":"DE"}}),
    )
    .await;
    assert_eq!("OFFERED", no_quantity["items"][0]["eligibility"]);
    assert!(no_quantity["items"][0]["options"][0]["charge"].is_null());
    assert!(
        no_quantity["items"][0]["options"][0]["reasons"]
            .as_array()
            .unwrap()
            .contains(&json!({"code":"MISSING_INPUT","input":"QUANTITY"}))
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
    let (_, cabinet) = request(&router, &path, None, explicit()).await;
    assert_eq!("NOT_OFFERED", cabinet["items"][0]["eligibility"]);
    assert_eq!("OFFERED", cabinet["items"][0]["collection"]["eligibility"]);
    assert_eq!(
        "BY_APPOINTMENT",
        cabinet["items"][0]["terms"]["collection"]["arrangement"]["kind"]
    );
    sqlx::query(
        "UPDATE product_listings SET delivery_configuration=NULL WHERE product_listing_id=$1",
    )
    .bind(id.into_uuid())
    .execute(&p)
    .await
    .unwrap();
    sqlx::query("UPDATE shipping_profiles SET fresh_until=verified_at,expires_at=now()+interval '1 hour' WHERE shipping_profile_id=$1").bind(profile.into_uuid()).execute(&p).await.unwrap();
    let (_, stale) = request(&router, &path, None, explicit()).await;
    assert_eq!("STALE", stale["items"][0]["profile"]["evidence"]["status"]);
    assert_eq!(
        updated["items"][0]["profile"]["evidence"]["verifiedAt"],
        stale["items"][0]["profile"]["evidence"]["verifiedAt"]
    );
    sqlx::query("UPDATE shipping_profiles SET expires_at=fresh_until WHERE shipping_profile_id=$1")
        .bind(profile.into_uuid())
        .execute(&p)
        .await
        .unwrap();
    let (_, expired) = request(&router, &path, None, explicit()).await;
    assert!(expired["items"][0]["eligibility"].is_null());
    assert_eq!(
        "EXPIRED",
        expired["items"][0]["profile"]["evidence"]["status"]
    );
    assert_eq!(
        json!(["PROFILE_EVIDENCE_EXPIRED"]),
        expired["items"][0]["notices"]
    );
}
#[aura_integration_test(services=[SCHEMA])]
async fn anonymous_explicit_none_preferences_owned_foreign_deleted_and_suspended() {
    let (p, router, book, owner, _, id) = fixture().await;
    let path = format!("/api/v1/product-listings/{id}/delivery-assessment");
    let context = ctx(Principal::User(owner));
    let address = book
        .create(
            &context,
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
    book.set_destination(&context, BrowsingDestination::Address(address.id), 1)
        .await
        .unwrap();
    let (_, anonymous) = request(&router, &path, None, json!({"input":{"quantity":1}})).await;
    assert!(anonymous["items"][0]["eligibility"].is_null());
    let (_, preferred) = request(
        &router,
        &path,
        Some("owner"),
        json!({"input":{"quantity":1}}),
    )
    .await;
    assert_eq!("OFFERED", preferred["items"][0]["eligibility"]);
    assert_eq!("DE", preferred["destination"]["submitted"]["country"]);
    assert!(!preferred.to_string().contains(&address.id.to_string()));
    let (_, none) = request(
        &router,
        &path,
        Some("owner"),
        json!({"destination":{"kind":"NONE"},"input":{"quantity":1}}),
    )
    .await;
    assert!(none["items"][0]["eligibility"].is_null());
    let body =
        json!({"destination":{"kind":"ADDRESS","addressId":address.id},"input":{"quantity":1}});
    assert_eq!(
        StatusCode::OK,
        request(&router, &path, Some("delegated"), body.clone())
            .await
            .0
    );
    assert_eq!(
        StatusCode::FORBIDDEN,
        request(&router, &path, Some("no-user-scope"), body.clone())
            .await
            .0
    );
    assert_eq!(
        StatusCode::NOT_FOUND,
        request(&router, &path, Some("other"), body.clone()).await.0
    );
    assert_eq!(
        StatusCode::UNAUTHORIZED,
        request(&router, &path, None, body.clone()).await.0
    );
    assert_eq!(
        StatusCode::UNAUTHORIZED,
        request(&router, &path, Some("invalid"), explicit()).await.0
    );
    assert_eq!(
        StatusCode::UNAUTHORIZED,
        request(
            &router,
            &path,
            None,
            json!({"destination":{"kind":"PREFERENCE"}})
        )
        .await
        .0
    );
    book.delete(&context, address.id, 1).await.unwrap();
    assert_eq!(
        StatusCode::NOT_FOUND,
        request(&router, &path, Some("owner"), body).await.0
    );
    // A captured value snapshot remains explainable after bookmark erasure.
    assert_eq!("DE", preferred["destination"]["effective"]["country"]);
    sqlx::query("UPDATE users SET suspended=true WHERE user_id=$1")
        .bind(owner.into_uuid())
        .execute(&p)
        .await
        .unwrap();
    assert_eq!(
        StatusCode::FORBIDDEN,
        request(&router, &path, Some("owner"), explicit()).await.0
    );
    assert_eq!(
        StatusCode::FORBIDDEN,
        request(
            &router,
            &path,
            Some("owner"),
            json!({"destination":{"kind":"NONE"}})
        )
        .await
        .0
    );
    assert_eq!(
        StatusCode::OK,
        request(&router, &path, None, explicit()).await.0
    );
}
#[aura_integration_test(services=[SCHEMA])]
async fn batch_limits_codecs_and_incompatible_variants_do_not_offer_or_calculate_charges() {
    let (_, router, _, _, _, id) = fixture().await;
    let path = format!("/api/v1/product-listings/{id}/delivery-assessment");
    for body in [
        json!({"destination":{"kind":"GEOGRAPHIC","country":"de"}}),
        json!({"destination":{"kind":"GEOGRAPHIC","country":"DE","subdivision":"FR-75"}}),
        json!({"destination":{"kind":"GEOGRAPHIC","country":"DE","postalCode":""}}),
        json!({"input":{"quantity":0}}),
        json!({"input":{"selectedVariant":{"listingSourceId":"invalid","code":"v"}}}),
        json!({"private":"unexpected"}),
    ] {
        assert_eq!(
            StatusCode::BAD_REQUEST,
            request(&router, &path, None, body).await.0
        );
    }
    let body = json!({"destination":{"kind":"GEOGRAPHIC","country":"DE"},"input":{"quantity":1,"selectedVariant":{"listingSourceId":ListingSourceId::new(),"code":"foreign"}}});
    let (_, result) = request(&router, &path, None, body).await;
    assert!(result["items"][0]["eligibility"].is_null());
    assert_eq!(json!([]), result["items"][0]["options"]);
    let batch = "/api/v1/product-listings/delivery-assessments";
    assert_eq!(
        StatusCode::BAD_REQUEST,
        request(&router, batch, None, json!({"listings":[]}))
            .await
            .0
    );
    assert_eq!(
        StatusCode::BAD_REQUEST,
        request(
            &router,
            batch,
            None,
            json!({"listings":vec![json!({"productListingId":id});21]})
        )
        .await
        .0
    );
    assert_eq!(
        StatusCode::BAD_REQUEST,
        request(
            &router,
            batch,
            None,
            json!({"listings":[{"productListingId":id},{"productListingId":id}]})
        )
        .await
        .0
    );
    let(status,result)=request(&router,batch,None,json!({"destination":{"kind":"GEOGRAPHIC","country":"DE"},"listings":[{"productListingId":id,"input":{"quantity":1}}]})).await;
    assert_eq!(StatusCode::OK, status);
    assert_eq!(1, result["items"].as_array().unwrap().len());
}

#[aura_integration_test(services=[SCHEMA])]
async fn current_location_eligibility_and_postal_exclusions_preserve_collection_privacy() {
    use shipping_core::{
        coverage::*, inputs::Applicability, terms::*, time::CollectionArrangement,
    };
    let (p, router, _, _, profile, id) = fixture().await;
    let path = format!("/api/v1/product-listings/{id}/delivery-assessment");
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
    let (_, active) = request(&router, &path, None, explicit()).await;
    assert_eq!("OFFERED", active["items"][0]["eligibility"]);
    assert!(active["items"][0]["terms"]["shipping"]["coverage"]["dispatch_origin"].is_null());
    assert!(active["items"][0]["terms"]["collection"]["site"].is_null());
    assert!(!active.to_string().contains("private pickup street"));
    assert!(!active.to_string().contains("private origin street"));
    sqlx::query("UPDATE party_locations SET lifecycle='RETIRED',revision=revision+1 WHERE party_location_id=$1").bind(site.into_uuid()).execute(&p).await.unwrap();
    let (_, retired) = request(&router, &path, None, explicit()).await;
    assert!(retired["items"][0]["eligibility"].is_null());
    assert_eq!(
        json!([{"code":"LOCATION_RETIRED"}]),
        retired["items"][0]["reasons"]
    );
    assert_eq!("OFFERED", retired["items"][0]["collection"]["eligibility"]);
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
    let (_, unknown) = request(&router, &path, None, explicit()).await;
    assert!(unknown["items"][0]["eligibility"].is_null());
    assert_eq!(json!([]), unknown["items"][0]["options"]);
    assert!(
        unknown["items"][0]["reasons"]
            .as_array()
            .unwrap()
            .contains(&json!({"code":"MISSING_INPUT","input":"POSTAL_CODE"}))
    );
    let(_,offered)=request(&router,&path,None,json!({"destination":{"kind":"GEOGRAPHIC","country":"DE","postalCode":"20000"},"input":{"quantity":1}})).await;
    assert_eq!("OFFERED", offered["items"][0]["eligibility"]);
}
