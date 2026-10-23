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
    fixture_with_projection().await.0
}
async fn fixture_with_projection() -> (
    (
        PgPool,
        axum::Router,
        Arc<Book>,
        UserId,
        ShippingProfileId,
        ProductListingId,
    ),
    IndexedFixture,
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
            import: None,
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
            import: None,
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
    let shipping = ShippingEnrichment::new(
        Arc::new(AssessDeliveryHandler::new(
            uow.clone(),
            SqlxAssessmentReaderFactory,
            book.clone(),
        )),
        Arc::new(fxrate_postgres::SqlxFxRateSnapshotReader::new(p.clone())),
    );
    let indexed_amount = Arc::new(std::sync::atomic::AtomicU64::new(8000));
    let indexed = IndexedFixture {
        amount: indexed_amount.clone(),
        item: ProductListingSearchItem {
            geography: None,
            product_listing_id: id,
            product_listing_title_slug_id:
                product_listing_core::product_listing_slug_id::ProductListingSlugId::raw(&format!(
                    "item-{}-abcdef",
                    id.as_uuid().simple()
                ))
                .unwrap(),
            event_id: domain_primitives::event_id::EventId::try_from(event).unwrap(),
            listing_source_id: source,
            source_listing_id: product_listing_core::source_listing_id::SourceListingId::try_from(
                format!("item-{}-abcdef", id.as_uuid().simple()),
            )
            .unwrap(),
            auction_id: None,
            title: None,
            display_price: None,
            price_valuation: ProductListingSummaryPriceValuation::Current {
                fx_rate_id: fxrate_core::FxRateId::try_from(fx).unwrap(),
                captured_at: time::OffsetDateTime::now_utc(),
            },
            availability: None,
            lifecycle: product_listing_core::listing_lifecycle::ListingLifecycle::Active,
            url: "https://example.test/item".parse().unwrap(),
            images: Default::default(),
            updated: time::OffsetDateTime::now_utc(),
        },
    };
    let search = SearchProductListingsHandler::new(
        indexed.clone(),
        fxrate_postgres::SqlxFxRateSnapshotReader::new(p.clone()),
        NoEmbeddings,
        product_listing_postgres::SqlxListingSourceSummaryReader::new(p.clone()),
        product_listing_postgres::SqlxProductListingUserStateReader::new(p.clone()),
        product_listing_postgres::SqlxProductListingContentAssessmentReader::new(p.clone()),
        shipping.clone(),
    );
    let router = app(AppState::new().with_products(ProductListingsState::new(
        Arc::new(GetProductListingHandler::new(
            uow,
            product_listing_postgres::SqlxProductListingDetailsReaderFactory::new(),
            fxrate_postgres::SqlxFxRateSnapshotRepositoryFactory,
            shipping,
        )),
        Arc::new(UnusedQueries),
        Arc::new(search),
        Arc::new(Auth(owner, other)),
    )));
    ((p, router, book, owner, profile.id, id), indexed)
}
/// Independently advanced indexed membership; the ordinary service still enriches from PostgreSQL.
#[derive(Clone)]
struct IndexedFixture {
    item: ProductListingSearchItem,
    amount: Arc<std::sync::atomic::AtomicU64>,
}
#[async_trait::async_trait]
impl product_listing_service::ports::ProductListingSearchReader for IndexedFixture {
    fn continuation_mode(
        &self,
        _: &Value,
        _: product_listing_core::sort_product_listing_field::SortProductListingField,
    ) -> Result<
        product_listing_service::ports::product_listing_search_reader::ProductListingSearchMode,
        product_listing_service::ports::ProductListingSearchReadError,
    > {
        Ok(product_listing_service::ports::product_listing_search_reader::ProductListingSearchMode::Lexical)
    }

    async fn search(
        &self,
        request: &product_listing_service::ports::ProductListingSearchReadRequest,
    ) -> Result<
        ProductListingSearchReadResult,
        product_listing_service::ports::ProductListingSearchReadError,
    > {
        let f = shipping_core::fixtures::gallery();
        let terms = shipping_core::fixtures::gallery_terms(
            self.amount.load(std::sync::atomic::Ordering::SeqCst),
        );
        let configuration = shipping_core::selection::ListingDeliveryConfiguration::new(
            f.source_id,
            shipping_core::selection::ProfileSelection::inline(terms),
            Default::default(),
            Default::default(),
        )
        .unwrap();
        let effective =
            shipping_core::assessment::EffectiveDeliveryTerms::resolve(&configuration, None, &[]);
        let within_dates = request
            .compiled_search
            .search
            .updated_query
            .as_ref()
            .is_none_or(|r| {
                r.min.is_none_or(|min| self.item.updated >= min)
                    && r.max.is_none_or(|max| self.item.updated <= max)
            });
        let matched = within_dates
            && request
                .compiled_search
                .search
                .shipping
                .as_ref()
                .is_none_or(|q| {
                    let assessment = effective.assess(
                        &shipping_core::inputs::AssessmentInput {
                            destination: q.country().and_then(|country| {
                                GeographicDescription::new(None, Some(country), None, None).unwrap()
                            }),
                            ..Default::default()
                        },
                        &[],
                    );
                    (!q.collection_offered()
                        || assessment.collection.eligibility
                            == Some(shipping_core::terms::ShippingEligibility::Offered))
                        && q.country().is_none_or(|_| {
                            assessment.services.iter().any(|offer| {
                                offer.applicability == shipping_core::assessment::PredicateResult::Match
                                    && offer.eligibility
                                        == Some(shipping_core::terms::ShippingEligibility::Offered)
                                    && q.service().is_none_or(|s| s == offer.service.code())
                                    && q.maximum_per_item().is_none_or(|maximum| {
                                        offer.service.rate().is_some_and(|rate| {
                                            rate.basis() == Some(shipping_core::pricing::ChargeBasis::PerItem)
                                                && match rate.price() {
                                                    shipping_core::pricing::ShippingPrice::Fixed(price) => {
                                                        if q.fx_converted() {
                                                            request.compiled_search.price_filter_plan.shipping_native_ranges(q).unwrap().unwrap().iter().any(|r|
                                                                r.source_currency == price.currency && u64::from(price.monetary_amount) >= r.lower
                                                                    && r.upper.is_none_or(|upper| u64::from(price.monetary_amount) <= upper))
                                                        } else {
                                                            price.currency == maximum.currency && price.monetary_amount <= maximum.monetary_amount
                                                        }
                                                    }
                                                    shipping_core::pricing::ShippingPrice::Free(currency) => {
                                                        currency == Some(maximum.currency)
                                                    }
                                                    _ => false,
                                                }
                                        })
                                    })
                            })
                        })
                });
        Ok(ProductListingSearchReadResult {
            items: if matched {
                vec![self.item.clone()]
            } else {
                vec![]
            },
            total: Some(u64::from(matched)),
            cursor: application::pagination::Cursor {
                size: 20,
                search_after: matched.then(|| json!([1.0, self.item.product_listing_id])),
            },
        })
    }
    async fn search_hybrid(
        &self,
        _: &product_listing_service::ports::ProductListingSearchReadRequest,
        _: &[f32],
    ) -> Result<
        ProductListingSearchReadResult,
        product_listing_service::ports::ProductListingSearchReadError,
    > {
        unreachable!()
    }
}
struct NoEmbeddings;
#[async_trait::async_trait]
impl embedding::EmbeddingGenerator for NoEmbeddings {
    async fn embed_product(
        &self,
        _: &embedding::EmbeddingText,
        _: Option<&embedding::EmbeddingText>,
        _: Option<&embedding::EmbeddingImageUrl>,
    ) -> Result<embedding::EmbeddingVector, embedding::EmbeddingError> {
        unreachable!()
    }
    async fn embed_search_query(
        &self,
        _: &embedding::EmbeddingText,
    ) -> Result<embedding::EmbeddingVector, embedding::EmbeddingError> {
        unreachable!()
    }
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
#[aura_integration_test(services=[SCHEMA], isolated = true)]
async fn listings_return_generic_terms_and_selected_destination_personalization() {
    let (_, router, book, owner, _, id) = fixture().await;
    let path = format!("/v1/product-listings/{id}");
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
        &format!("/v1/product-listings/by-slug/{slug}"),
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
#[aura_integration_test(services=[SCHEMA], isolated = true)]
async fn current_profile_changes_overrides_observation_age_and_suspension_are_reflected() {
    let (p, router, book, owner, profile, id) = fixture().await;
    select_country(&book, owner, None, 1).await;
    let path = format!("/v1/product-listings/{id}");
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
            import: None,
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
    // Observation age never withdraws advertised shipping or shortens normal discovery caching.
    sqlx::query("UPDATE shipping_profiles SET verified_at=now()-interval '3650 days' WHERE shipping_profile_id=$1")
        .bind(profile.into_uuid()).execute(&p).await.unwrap();
    let anonymous = router
        .clone()
        .oneshot(Request::get(&path).body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(StatusCode::OK, anonymous.status());
    let cache = anonymous.headers()["cache-control"].to_str().unwrap();
    assert!(cache.contains("s-maxage=120"), "{cache}");
    let (_, old) = request(&router, &path, Some("owner")).await;
    assert_eq!("OFFERED", old["userState"]["shipping"]["eligibility"]);
    let evidence = old["item"]["shipping"]["profile"]["evidence"]
        .as_object()
        .unwrap();
    assert_eq!(1, evidence.len());
    assert!(evidence.contains_key("verifiedAt"));
    assert!(old["item"]["shipping"].get("validityDeadline").is_none());
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
        format!("/v1/product-listings/{id}/delivery-assessment"),
        "/v1/product-listings/delivery-assessments".into(),
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
#[aura_integration_test(services=[SCHEMA], isolated = true)]
async fn current_location_eligibility_and_postal_exclusions_preserve_collection_privacy() {
    use shipping_core::{
        coverage::*, inputs::Applicability, terms::*, time::CollectionArrangement,
    };
    let (p, router, book, user, profile, id) = fixture().await;
    select_country(&book, user, None, 1).await;
    let path = format!("/v1/product-listings/{id}");
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
                import: None,
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
                import: None,
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

struct NoDiscoveryEmbedding;
#[async_trait::async_trait]
impl embedding::EmbeddingGenerator for NoDiscoveryEmbedding {
    async fn embed_product(
        &self,
        _: &embedding::EmbeddingText,
        _: Option<&embedding::EmbeddingText>,
        _: Option<&embedding::EmbeddingImageUrl>,
    ) -> Result<embedding::EmbeddingVector, embedding::EmbeddingError> {
        unreachable!("shipping discovery never calls an embedding provider")
    }
    async fn embed_search_query(
        &self,
        _: &embedding::EmbeddingText,
    ) -> Result<embedding::EmbeddingVector, embedding::EmbeddingError> {
        unreachable!("country discovery needs no text embedding")
    }
}
#[aura_integration_test(services=[SCHEMA, test_api::OpenSearch()], isolated = true)]
async fn origin_search_reads_current_shipping_before_index_catchup_and_preserves_index_membership(
    context: test_api::TestContext,
) {
    use application::transaction::{Transaction, UnitOfWork};
    use product_listing_postgres::{
        SqlxProductListingSearchFilterMatchSourceReaderFactory,
        shipping_projection::SqlxShippingProjectionStoreFactory,
    };
    use product_listing_service::{
        ports::{
            ProductListingSearchFilterMatchSourceReader,
            ProductListingSearchFilterMatchSourceReaderFactory, ProductListingSearchProjection,
        },
        shipping_projection::*,
    };
    let (p, _, book, owner, profile, id) = fixture().await;
    select_country(&book, owner, None, 1).await;
    let client = test_api::get_opensearch_client().await;
    let index = context.product_listings_index().expect("search index");
    let shipping = ShippingEnrichment::new(
        Arc::new(AssessDeliveryHandler::new(
            SqlxUnitOfWork::new(p.clone()),
            SqlxAssessmentReaderFactory,
            book,
        )),
        Arc::new(fxrate_postgres::SqlxFxRateSnapshotReader::new(p.clone())),
    );
    let search = SearchProductListingsHandler::new(
        product_listing_opensearch::OpenSearchProductListingSearchReader::with_index(
            client.clone(),
            index.clone(),
        ),
        fxrate_postgres::SqlxFxRateSnapshotReader::new(p.clone()),
        NoDiscoveryEmbedding,
        product_listing_postgres::SqlxListingSourceSummaryReader::new(p.clone()),
        product_listing_postgres::SqlxProductListingUserStateReader::new(p.clone()),
        product_listing_postgres::SqlxProductListingContentAssessmentReader::new(p.clone()),
        shipping.clone(),
    );
    let router = app(AppState::new().with_products(ProductListingsState::new(
        Arc::new(GetProductListingHandler::new(
            SqlxUnitOfWork::new(p.clone()),
            product_listing_postgres::SqlxProductListingDetailsReaderFactory::new(),
            fxrate_postgres::SqlxFxRateSnapshotRepositoryFactory,
            shipping,
        )),
        Arc::new(UnusedQueries),
        Arc::new(search),
        Arc::new(Auth(owner, UserId::new())),
    )));
    let project = ProjectListingShippingHandler::new(
        SqlxUnitOfWork::new(p.clone()),
        SqlxShippingProjectionStoreFactory,
    );
    async fn drain(project: &dyn ProjectListingShippingUseCase, p: &PgPool) {
        for _ in 0..100 {
            let work: Option<uuid::Uuid> = sqlx::query_scalar("SELECT work_id FROM listing_shipping_work WHERE pending ORDER BY pending_since,work_id LIMIT 1")
                .fetch_optional(p).await.unwrap();
            let Some(work) = work else {
                return;
            };
            project
                .execute(work.try_into().unwrap(), 100)
                .await
                .unwrap();
            if !sqlx::query_scalar::<_, bool>(
                "SELECT EXISTS(SELECT 1 FROM listing_shipping_work WHERE pending)",
            )
            .fetch_one(p)
            .await
            .unwrap()
            {
                return;
            }
        }
        panic!("shipping work did not converge");
    }
    async fn source(
        p: &PgPool,
        id: ProductListingId,
    ) -> product_listing_service::ports::ProductListingSearchFilterMatchSource {
        let event: uuid::Uuid = sqlx::query_scalar(
            "SELECT current_event_id FROM product_listings WHERE product_listing_id=$1",
        )
        .bind(id.into_uuid())
        .fetch_one(p)
        .await
        .unwrap();
        let mut tx = SqlxUnitOfWork::new(p.clone()).begin().await.unwrap();
        let source = SqlxProductListingSearchFilterMatchSourceReaderFactory
            .in_transaction(&mut tx)
            .find_source(event.try_into().unwrap(), id)
            .await
            .unwrap()
            .unwrap();
        tx.commit().await.unwrap();
        source
    }
    async fn old_rate_membership(client: &opensearch::OpenSearch, index: &str) -> Value {
        client.search(opensearch::SearchParts::Index(&[index]))
            .body(json!({"query":{"bool":{"filter":[product_listing_opensearch::shipping_query::country_offer_query(CountryCode::DEU,None,Some(money::Price::new(8000_u64.into(),money::Currency::Eur)))],"must_not":[{"term":{"projectionDeleted":true}}]}}}))
            .send().await.unwrap().error_for_status_code().unwrap().json().await.unwrap()
    }
    // Public title is unrelated to shipping; supply it so this is an ordinary search hit.
    sqlx::query("UPDATE product_listings SET title_text='Gallery painting',title_language='en' WHERE product_listing_id=$1").bind(id.into_uuid()).execute(&p).await.unwrap();
    drain(&project, &p).await;
    let writer = product_listing_opensearch::OpenSearchProductListingSearchProjection::with_index(
        client.clone(),
        index.clone(),
    );
    writer.upsert(&source(&p, id).await, None).await.unwrap();
    test_api::refresh_index(&index).await;
    assert_eq!(
        1,
        old_rate_membership(&client, &index).await["hits"]["total"]["value"]
    );
    let (status, before) = request(&router, "/v1/product-listings", Some("owner")).await;
    assert_eq!(StatusCode::OK, status, "{before}");
    let source_id: ListingSourceId = sqlx::query_scalar::<_, uuid::Uuid>(
        "SELECT listing_source_id FROM product_listings WHERE product_listing_id=$1",
    )
    .bind(id.into_uuid())
    .fetch_one(&p)
    .await
    .unwrap()
    .try_into()
    .unwrap();
    ReplaceShippingProfileHandler::new(
        SqlxUnitOfWork::new(p.clone()),
        SqlxShippingRepositoryFactory,
        SqlxSourceLocationAccessFactory,
    )
    .execute(
        &ctx(Principal::System),
        ReplaceProfileCommand {
            import: None,
            source_id,
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
    let (status, current) = request(&router, "/v1/product-listings", Some("owner")).await;
    assert_eq!(StatusCode::OK, status, "{current}");
    assert_eq!(1, current["size"]);
    assert_eq!(1, current["total"]);
    assert_eq!(
        id.to_string(),
        current["items"][0]["item"]["productListingId"]
    );
    assert_eq!(before["total"], current["total"]);
    assert_eq!(before["size"], current["size"]);
    assert_eq!(
        before["items"][0]["item"]["productListingId"],
        current["items"][0]["item"]["productListingId"]
    );
    assert_eq!(
        9500,
        current["items"][0]["item"]["shipping"]["terms"]["shipping"]["coverage"]["zones"][0]["services"]
            [0]["rate"]["price"]["amount"]["amount_minor"]
    );
    assert_eq!(
        "OFFERED",
        current["items"][0]["userState"]["shipping"]["eligibility"]
    );
    // PostgreSQL is current while indexed membership/counts retain the old predicate.
    assert_eq!(
        1,
        old_rate_membership(&client, &index).await["hits"]["total"]["value"]
    );
    drain(&project, &p).await;
    writer.upsert(&source(&p, id).await, None).await.unwrap();
    test_api::refresh_index(&index).await;
    assert_eq!(
        0,
        old_rate_membership(&client, &index).await["hits"]["total"]["value"]
    );
    let (_, caught_up) = request(&router, "/v1/product-listings", Some("owner")).await;
    assert_eq!(current["total"], caught_up["total"]);
    assert_eq!(current["size"], caught_up["size"]);
    assert_eq!(
        current["items"][0]["item"]["shipping"]["terms"],
        caught_up["items"][0]["item"]["shipping"]["terms"]
    );
}

#[aura_integration_test(services=[SCHEMA], isolated = true)]
async fn search_country_is_independent_of_browsing_and_current_shipping_during_projection_lag() {
    let ((p, router, book, owner, profile, _), indexed) = fixture_with_projection().await;
    let path = "/v1/product-listings?shipping[destination][kind]=COUNTRY&shipping[destination][country]=DE&shipping[maximumPerItem][amount]=9000&shipping[maximumPerItem][currency]=EUR";
    let (status, before) = request(&router, path, None).await;
    assert_eq!(StatusCode::OK, status, "{before}");
    assert_eq!(1, before["total"]);
    assert!(before["items"][0].get("userState").is_none());
    let (_, unselected) = request(&router, path, Some("owner")).await;
    assert!(
        unselected["items"][0]["userState"]
            .get("shipping")
            .is_none()
    );
    book.set_destination(
        &ctx(Principal::User(owner)),
        BrowsingDestination::CountryPostal {
            country: CountryCode::FRA,
            postal_code: Some(geo::PostalCode::new("75001").unwrap()),
        },
        1,
    )
    .await
    .unwrap();
    let (_, selected) = request(&router, path, Some("owner")).await;
    assert_eq!(
        "FR",
        selected["items"][0]["userState"]["shipping"]["destination"]["country"]
    );
    let (_, delegated) = request(&router, path, Some("no-user-scope")).await;
    assert!(delegated["items"][0]["userState"].get("shipping").is_none());
    let terms: Value =
        sqlx::query_scalar("SELECT terms FROM shipping_profiles WHERE shipping_profile_id=$1")
            .bind(profile.into_uuid())
            .fetch_one(&p)
            .await
            .unwrap();
    let mut changed = terms;
    changed["shipping"]["coverage"]["zones"][0]["services"][0]["rate"]["price"]["amount"]["amount_minor"] =
        json!(9500);
    sqlx::query("UPDATE shipping_profiles SET terms=$2,management_revision=management_revision+1,terms_revision=terms_revision+1,verified_at=now()-interval '365 days' WHERE shipping_profile_id=$1").bind(profile.into_uuid()).bind(changed).execute(&p).await.unwrap();
    let (_, lagging) = request(&router, path, None).await;
    assert_eq!(1, lagging["total"]);
    assert_eq!(before["searchAfter"], lagging["searchAfter"]);
    assert_eq!(
        before["items"][0]["item"]["productListingId"],
        lagging["items"][0]["item"]["productListingId"]
    );
    assert_eq!(
        9500,
        lagging["items"][0]["item"]["shipping"]["terms"]["shipping"]["coverage"]["zones"][0]["services"]
            [0]["rate"]["price"]["amount"]["amount_minor"]
    );
    indexed
        .amount
        .store(9500, std::sync::atomic::Ordering::SeqCst);
    let (_, converged) = request(&router, path, None).await;
    assert_eq!(0, converged["total"]);
    assert!(converged["items"].as_array().unwrap().is_empty());
    let (_, collection) = request(
        &router,
        "/v1/product-listings?shipping[collectionOffered]=true",
        None,
    )
    .await;
    assert_eq!(1, collection["total"]);
    let destination = book
        .destination(&ctx(Principal::User(owner)))
        .await
        .unwrap();
    assert_eq!(
        BrowsingDestination::CountryPostal {
            country: CountryCode::FRA,
            postal_code: Some(geo::PostalCode::new("75001").unwrap())
        },
        destination.destination
    );
}

struct SingleTestRun;
#[async_trait::async_trait]
impl search_filter_service::ports::PeriodicSearchFilterMatchingRunLease for SingleTestRun {
    async fn release(
        self: Box<Self>,
    ) -> Result<(), search_filter_service::ports::PeriodicSearchFilterMatchingRunLockError> {
        Ok(())
    }
}
#[async_trait::async_trait]
impl search_filter_service::ports::PeriodicSearchFilterMatchingRunLock for SingleTestRun {
    async fn try_acquire(
        &self,
    ) -> Result<
        Option<Box<dyn search_filter_service::ports::PeriodicSearchFilterMatchingRunLease>>,
        search_filter_service::ports::PeriodicSearchFilterMatchingRunLockError,
    > {
        Ok(Some(Box::new(Self)))
    }
}
#[async_trait::async_trait]
impl classifier_model::ClassifierModel for NoEmbeddings {
    async fn classify(
        &self,
        _: classifier_model::ClassificationRequest,
    ) -> Result<classifier_model::ClassificationResponse, classifier_model::ClassificationError>
    {
        unreachable!("structured shipping filters need no model calls")
    }
}
async fn replace_gallery(pool: &PgPool, profile: ShippingProfileId, amount: u64) {
    let (source, revision): (uuid::Uuid, i64) = sqlx::query_as("SELECT listing_source_id,management_revision FROM shipping_profiles WHERE shipping_profile_id=$1").bind(profile.into_uuid()).fetch_one(pool).await.unwrap();
    ReplaceShippingProfileHandler::new(
        SqlxUnitOfWork::new(pool.clone()),
        SqlxShippingRepositoryFactory,
        SqlxSourceLocationAccessFactory,
    )
    .execute(
        &ctx(Principal::System),
        ReplaceProfileCommand {
            import: None,
            source_id: ListingSourceId::try_from(source).unwrap(),
            profile_id: profile,
            expected_revision: shipping_service::ports::ShippingManagementRevision::try_from(
                revision,
            )
            .unwrap(),
            content: ShippingProfileContent {
                name: ShippingProfileName::new("Gallery terms".into()).unwrap(),
                attributed_party: None,
                terms: gallery_terms(amount),
            },
            evidence: vec![],
        },
    )
    .await
    .unwrap();
}
async fn project_gallery(pool: &PgPool, index: &mut IndexedFixture) {
    use product_listing_service::shipping_projection::*;
    let handler = ProjectListingShippingHandler::new(
        SqlxUnitOfWork::new(pool.clone()),
        product_listing_postgres::shipping_projection::SqlxShippingProjectionStoreFactory,
    );
    for _ in 0..20 {
        let work: Option<uuid::Uuid> = sqlx::query_scalar(
            "SELECT work_id FROM listing_shipping_work WHERE pending ORDER BY work_id LIMIT 1",
        )
        .fetch_optional(pool)
        .await
        .unwrap();
        let Some(work) = work else {
            let value: Value = sqlx::query_scalar(
                "SELECT summary FROM product_listing_shipping WHERE product_listing_id=$1",
            )
            .bind(index.item.product_listing_id.into_uuid())
            .fetch_one(pool)
            .await
            .unwrap();
            index.amount.store(
                value["country_offers"][0]["per_item_fixed"]["amount_minor"]
                    .as_u64()
                    .unwrap(),
                std::sync::atomic::Ordering::SeqCst,
            );
            let event: uuid::Uuid = sqlx::query_scalar(
                "SELECT current_event_id FROM product_listings WHERE product_listing_id=$1",
            )
            .bind(index.item.product_listing_id.into_uuid())
            .fetch_one(pool)
            .await
            .unwrap();
            index.item.event_id = domain_primitives::event_id::EventId::try_from(event).unwrap();
            // Carry the persisted freshness with the state, exactly as the OpenSearch projector
            // reads it for the complete document.
            let source = {
                use application::transaction::{Transaction, UnitOfWork};
                use product_listing_service::ports::{
                    ProductListingSearchFilterMatchSourceReader,
                    ProductListingSearchFilterMatchSourceReaderFactory,
                };
                let mut tx = SqlxUnitOfWork::new(pool.clone()).begin().await.unwrap();
                let source =
                    product_listing_postgres::SqlxProductListingSearchFilterMatchSourceReaderFactory
                        .in_transaction(&mut tx)
                        .find_source(index.item.event_id, index.item.product_listing_id)
                        .await
                        .unwrap()
                        .unwrap();
                tx.commit().await.unwrap();
                source
            };
            assert_eq!(index.item.event_id, source.current_event_id);
            index.item.updated = source.updated;
            return;
        };
        handler
            .execute(ShippingWorkId::try_from(work).unwrap(), 100)
            .await
            .unwrap();
    }
    panic!("bounded shipping projection did not converge");
}
#[aura_integration_test(services=[SCHEMA], isolated = true)]
async fn profile_change_discovers_old_listing_on_next_periodic_evaluation_without_canonical_events()
{
    periodic_shipping_change(false).await;
}

#[aura_integration_test(services=[SCHEMA], isolated = true)]
async fn fx_change_alone_does_not_discover_old_listing_outside_the_updated_window() {
    periodic_shipping_change(true).await;
}

async fn periodic_shipping_change(fx_mode: bool) {
    use search_filter_postgres::*;
    use search_filter_service::use_cases::*;
    let ((pool, _, _, owner, profile, listing), mut index) = fixture_with_projection().await;
    sqlx::query("UPDATE users SET tier='PRO' WHERE user_id=$1")
        .bind(owner.into_uuid())
        .execute(&pool)
        .await
        .unwrap();
    let profile_before: Value = sqlx::query_scalar(
        "SELECT to_jsonb(p) FROM shipping_profiles p WHERE shipping_profile_id=$1",
    )
    .bind(profile.into_uuid())
    .fetch_one(&pool)
    .await
    .unwrap();
    let old = time::OffsetDateTime::now_utc() - time::Duration::days(100);
    let old = old
        .replace_nanosecond(old.nanosecond() / 1_000_000 * 1_000_000)
        .unwrap();
    index.item.updated = old;
    sqlx::query("UPDATE product_listings SET updated=$2 WHERE product_listing_id=$1")
        .bind(listing.into_uuid())
        .bind(old)
        .execute(&pool)
        .await
        .unwrap();
    let domain_events: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM product_listing_events WHERE event_group='DOMAIN'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    let listing_before: (i64, i64, uuid::Uuid, i64) = sqlx::query_as(
        "SELECT version,projection_version,current_event_id,(SELECT count(*) FROM product_listing_events e WHERE e.product_listing_id=p.product_listing_id) FROM product_listings p WHERE product_listing_id=$1",
    )
    .bind(listing.into_uuid())
    .fetch_one(&pool)
    .await
    .unwrap();
    let search = product_listing_core::product_listing_search::ProductListingSearch {
        shipping: Some(
            product_listing_core::shipping_search::ShippingSearch::new(
                geo::reference::ReferenceRelease::CURRENT,
                Some(CountryCode::DEU),
                None,
                Some(money::Price::new(
                    if fx_mode {
                        7500_u64.into()
                    } else {
                        9000_u64.into()
                    },
                    if fx_mode {
                        money::Currency::Usd
                    } else {
                        money::Currency::Eur
                    },
                )),
                false,
            )
            .and_then(|query| {
                if fx_mode {
                    query.with_fx_conversion()
                } else {
                    Ok(query)
                }
            })
            .unwrap(),
        ),
        ..Default::default()
    };
    let filter = CreateSearchFilterHandler::new(
        SqlxUnitOfWork::new(pool.clone()),
        SqlxSearchFilterRepositoryFactory,
        NoEmbeddings,
        SqlxSearchFilterQuotaReaderFactory,
        user_postgres::SqlxUserTierEntitlementsFactory::new(),
    )
    .execute(
        &ctx(Principal::User(owner)),
        CreateSearchFilterCommand {
            accept_shipping_destination: None,
            user_id: owner,
            name: "definite delivery".into(),
            notifications: false,
            search,
        },
    )
    .await
    .unwrap()
    .filter;
    sqlx::query(
        "UPDATE search_filters SET created=now()-interval '31 days' WHERE user_search_filter_id=$1",
    )
    .bind(filter.search_filter_id.into_uuid())
    .execute(&pool)
    .await
    .unwrap();
    let started = time::OffsetDateTime::now_utc() + time::Duration::seconds(1);
    // Profile changes advance listing freshness, so the ordinary interval rediscovers the old
    // listing. FX changes alone never widen the interval, even when the converted shipping price
    // would now satisfy the threshold.
    let mut freshness = Vec::new();
    for (iteration, amount, expected_matches) in [(0, 9500, 0), (1, 8000, 1), (2, 8000, 0)] {
        let expected_matches = if fx_mode { 0 } else { expected_matches };
        if fx_mode {
            capture_shipping_fx(&pool, if iteration == 0 { 1_000_000 } else { 900_000 }).await;
        } else {
            replace_gallery(&pool, profile, amount).await;
            project_gallery(&pool, &mut index).await;
        }
        let updated: time::OffsetDateTime =
            sqlx::query_scalar("SELECT updated FROM product_listings WHERE product_listing_id=$1")
                .bind(listing.into_uuid())
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(updated, index.item.updated);
        freshness.push(updated);
        let handler = RunPeriodicSearchFilterMatchingHandler::new(
            SqlxUnitOfWork::new(pool.clone()),
            SingleTestRun,
            SqlxPeriodicSearchFilterCandidateReader::new(pool.clone()),
            fxrate_postgres::SqlxFxRateSnapshotRepositoryFactory,
            index.clone(),
            SqlxExistingSearchFilterMatchReader::new(pool.clone()),
            product_listing_postgres::SqlxProductListingSearchFilterMatchSourceReaderFactory::new(),
            NoEmbeddings,
            product_listing_postgres::SqlxProductListingCurrentEventGuardFactory::new(),
            SqlxSearchFilterMatchWriterFactory,
            SqlxPeriodicSearchFilterProgressFactory,
            notification_postgres::SqlxSavedSearchDigestStoreFactory,
            PeriodicSearchFilterMatchingPolicy {
                filter_page_size: std::num::NonZeroUsize::new(100).unwrap(),
                hybrid_scan_limit: std::num::NonZeroUsize::new(100).unwrap(),
                evaluation_limit: std::num::NonZeroUsize::new(50).unwrap(),
                classification_concurrency: std::num::NonZeroUsize::MIN,
                classification_batch_candidates: std::num::NonZeroUsize::MIN,
                should_show_threshold: classifier_model::Probability::new(0.5).unwrap(),
                max_attempts: std::num::NonZeroUsize::MIN,
                projection_lag: time::Duration::ZERO,
                replay_overlap: time::Duration::hours(2),
            },
        )
        .unwrap();
        let outcome = handler
            .execute(RunPeriodicSearchFilterMatchingCommand {
                started_at: started + time::Duration::hours(iteration),
            })
            .await
            .unwrap();
        let RunPeriodicSearchFilterMatchingOutcome::Applied(report) = outcome else {
            panic!("single test run should acquire custody")
        };
        assert_eq!(0, report.filters_failed, "{report:?}");
        assert_eq!(expected_matches, report.matches_inserted, "{report:?}");
        assert_eq!(0, report.classification_requests);
    }
    let persisted: i64 = sqlx::query_scalar("SELECT count(*) FROM search_filter_matches WHERE user_search_filter_id=$1 AND product_listing_id=$2").bind(filter.search_filter_id.into_uuid()).bind(listing.into_uuid()).fetch_one(&pool).await.unwrap();
    assert_eq!(i64::from(!fx_mode), persisted);
    let after: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM product_listing_events WHERE event_group='DOMAIN'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(domain_events, after);
    let listing_after: (i64, i64, uuid::Uuid, i64) = sqlx::query_as(
        "SELECT version,projection_version,current_event_id,(SELECT count(*) FROM product_listing_events e WHERE e.product_listing_id=p.product_listing_id) FROM product_listings p WHERE product_listing_id=$1",
    )
    .bind(listing.into_uuid())
    .fetch_one(&pool)
    .await
    .unwrap();
    // Enrichment never manufactures a canonical revision.
    assert_eq!(listing_before.0, listing_after.0);
    if fx_mode {
        // FX capture leaves listing state, freshness and projection evidence untouched.
        assert_eq!(listing_before, listing_after);
        assert!(freshness.iter().all(|updated| *updated == old));
        let profile_after: Value = sqlx::query_scalar(
            "SELECT to_jsonb(p) FROM shipping_profiles p WHERE shipping_profile_id=$1",
        )
        .bind(profile.into_uuid())
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(profile_before, profile_after);
        let digests: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM saved_search_digest_members m JOIN search_filter_matches f USING(user_search_filter_id,product_listing_id) WHERE f.user_search_filter_id=$1",
        )
        .bind(filter.search_filter_id.into_uuid())
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(0, digests);
    } else {
        // Each effective summary change advances freshness strictly; an unchanged effective
        // result (the repeated 8000 replacement) preserves it and the projection fence.
        assert!(old < freshness[0] && freshness[0] < freshness[1]);
        assert_eq!(freshness[1], freshness[2]);
        assert_eq!(listing_before.1 + 2, listing_after.1);
    }
}

async fn capture_shipping_fx(pool: &PgPool, usd_units: i64) -> fxrate_core::FxRateId {
    let id = fxrate_core::FxRateId::new();
    let mut tx = pool.begin().await.unwrap();
    sqlx::query("INSERT INTO fx_rates(fx_rate_id,captured_at,source,source_event_id)VALUES($1,now(),'fxratesapi',$2)")
        .bind(id.into_uuid()).bind(id.to_string()).execute(&mut *tx).await.unwrap();
    sqlx::query("INSERT INTO fx_rate_quotes(fx_rate_id,currency,units_per_eur) SELECT $1,currency,CASE WHEN currency='USD' THEN $2 ELSE units_per_eur END FROM fx_rate_quotes WHERE fx_rate_id=(SELECT fx_rate_id FROM fx_rates WHERE fx_rate_id<>$1 ORDER BY captured_at DESC,generation DESC LIMIT 1)")
        .bind(id.into_uuid()).bind(usd_units).execute(&mut *tx).await.unwrap();
    tx.commit().await.unwrap();
    id
}

#[aura_integration_test(services=[SCHEMA], isolated = true)]
async fn shipping_display_and_explicit_fx_filter_use_current_snapshot_and_pin_continuation() {
    let (pool, router, book, owner, _, id) = fixture().await;
    select_country(&book, owner, None, 1).await;
    let query = "/v1/product-listings?currency=USD&shipping[destination][kind]=COUNTRY&shipping[destination][country]=DE&shipping[interpretation]=WHOLE_COUNTRY_PER_ITEM_FX_V1&shipping[maximumPerItem][amount]=9000&shipping[maximumPerItem][currency]=USD";
    let (status, first) = request(&router, query, None).await;
    assert_eq!(StatusCode::OK, status, "{first}");
    assert_eq!(1, first["total"]);
    let first_shipping = &first["items"][0]["item"]["shipping"];
    assert_eq!(
        8000,
        first_shipping["display"]["rates"][0]["price"]["amount"]["amount_minor"]
    );
    let encoded_cursor: String =
        url::form_urlencoded::byte_serialize(first["searchAfter"].to_string().as_bytes()).collect();
    capture_shipping_fx(&pool, 1_200_000).await;
    let (status, continued) = request(
        &router,
        &format!("{query}&searchAfter={encoded_cursor}"),
        None,
    )
    .await;
    assert_eq!(StatusCode::OK, status, "{continued}");
    assert_eq!(
        first_shipping["display"],
        continued["items"][0]["item"]["shipping"]["display"]
    );
    assert_eq!(1, continued["total"]);
    let (_, changed_membership) = request(&router, query, None).await;
    assert_eq!(0, changed_membership["total"]);
    let fx = capture_shipping_fx(&pool, 900_000).await;
    let threshold = query.replace("amount]=9000", "amount]=7500");
    let (_, fresh) = request(&router, &threshold, Some("owner")).await;
    assert_eq!(1, fresh["total"]);
    let shipping = &fresh["items"][0]["item"]["shipping"];
    assert_eq!(fx.to_string(), shipping["display"]["fxRateId"]);
    assert_eq!("USD", shipping["display"]["currency"]);
    assert_eq!(
        7200,
        shipping["display"]["rates"][0]["price"]["amount"]["amount_minor"]
    );
    assert_eq!(first_shipping["terms"], shipping["terms"]);
    assert_eq!(
        "EUR",
        shipping["terms"]["shipping"]["coverage"]["zones"][0]["services"][0]["rate"]["price"]["amount"]
            ["currency"]
    );
    let (_, native) = request(
        &router,
        &threshold.replace("WHOLE_COUNTRY_PER_ITEM_FX_V1", "WHOLE_COUNTRY_PER_ITEM_V1"),
        None,
    )
    .await;
    assert_eq!(0, native["total"]);
    let (_, eur) = request(
        &router,
        &format!("/v1/product-listings/{id}?currency=EUR"),
        None,
    )
    .await;
    let (_, usd) = request(
        &router,
        &format!("/v1/product-listings/{id}?currency=USD"),
        None,
    )
    .await;
    assert_eq!(
        8000,
        eur["item"]["shipping"]["display"]["rates"][0]["price"]["amount"]["amount_minor"]
    );
    assert_eq!(
        7200,
        usd["item"]["shipping"]["display"]["rates"][0]["price"]["amount"]["amount_minor"]
    );
    assert_eq!(
        eur["item"]["shipping"]["terms"],
        usd["item"]["shipping"]["terms"]
    );
    // Sold item pricing retains its observation basis; current advertised shipping does not.
    let old_fx: uuid::Uuid = sqlx::query_scalar("SELECT fx_rate_id FROM fx_rates WHERE fx_rate_id<>$1 ORDER BY captured_at DESC,generation DESC LIMIT 1")
        .bind(fx.into_uuid()).fetch_one(&pool).await.unwrap();
    sqlx::query("UPDATE product_listings SET availability='SOLD_OUT', sale_observed_at=now(), sale_observation_fx_rate_id=$2 WHERE product_listing_id=$1")
        .bind(id.into_uuid()).bind(old_fx).execute(&pool).await.unwrap();
    let (status, sold) = request(
        &router,
        &format!("/v1/product-listings/{id}?currency=USD"),
        None,
    )
    .await;
    assert_eq!(StatusCode::OK, status, "{sold}");
    assert_eq!(
        fx.to_string(),
        sold["item"]["shipping"]["display"]["fxRateId"]
    );
}
