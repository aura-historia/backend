use super::*;
use aura_historia_runtime::Runtime;
use test_api::{IntegrationTestService, aura_integration_test, get_postgres_client};
const SCHEMA: test_api::Postgres = test_api::Postgres::new("migrations");
async fn seed_source(pool: &sqlx::PgPool) -> (uuid::Uuid, ListingSourceId) {
    let party = uuid::Uuid::now_v7();
    let source = ListingSourceId::new();
    sqlx::query("INSERT INTO parties(party_id,party_slug_id,name) VALUES($1,$2,'Operator')")
        .bind(party)
        .bind(format!("party-{party}"))
        .execute(pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO listing_sources(listing_source_id,listing_source_slug_id,name,operator_party_id) VALUES($1,$2,'Partner source',$3)").bind(source.into_uuid()).bind(format!("source-{}",source.as_uuid())).bind(party).execute(pool).await.unwrap();
    (party, source)
}
async fn seed_listing(pool: &sqlx::PgPool, source: ListingSourceId) -> ProductListingId {
    use application::transaction::{Transaction, UnitOfWork};
    use platform_postgres::SqlxUnitOfWork;
    use product_listing_core::product_listing::{NewProductListing, ProductListing};
    use product_listing_postgres::{
        SqlxProductListingEventAppenderFactory, SqlxProductListingRepositoryFactory,
    };
    use product_listing_service::ports::{
        ProductListingEventAppender, ProductListingEventAppenderFactory, ProductListingRepository,
        ProductListingRepositoryFactory, stamp_product_listing_event,
    };
    let id = ProductListingId::new();
    let mut listing = ProductListing::create(NewProductListing {
        id,
        title_slug_id: ProductListingSlugId::from_title_and_suffix(
            &format!("Original {}", id.as_uuid()),
            "a1b2c3",
        )
        .unwrap(),
        listing_source_id: source,
        source_listing_id: SourceListingId::try_from(id.to_string()).unwrap(),
        title: Some(Localized::new(Language::En, Title::from("Original"))),
        description: Some(Localized::new(Language::En, Description::from("Retained"))),
        pricing: ProductListingPricing::default(),
        availability: None,
        url: Url::parse("https://source.test/listing").unwrap(),
        images: std::iter::once(ProductListingImage::new(
            Url::parse("https://source.test/original.jpg").unwrap(),
        ))
        .collect(),
        auction: None,
    })
    .unwrap();
    let event = stamp_product_listing_event(
        id,
        OffsetDateTime::now_utc(),
        listing.take_pending_event_payload().unwrap(),
    );
    let mut tx = SqlxUnitOfWork::new(pool.clone()).begin().await.unwrap();
    SqlxProductListingRepositoryFactory::new()
        .in_transaction(&mut tx)
        .insert(&listing, event.event_id)
        .await
        .unwrap();
    SqlxProductListingEventAppenderFactory::new()
        .in_transaction(&mut tx)
        .append(&event)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    id
}
async fn error(
    registry: &Registry,
    name: &str,
    input: Value,
    context: InvocationContext,
) -> OperationError {
    registry
        .get(name)
        .unwrap()
        .call(input, context)
        .await
        .unwrap_err()
}

struct DatabaseFixture {
    pool: sqlx::PgPool,
    registry: Registry,
    context: InvocationContext,
    source: ListingSourceId,
    id: ProductListingId,
    foreign_id: ProductListingId,
    foreign_source: ListingSourceId,
    foreign_party: uuid::Uuid,
    user_id: user_core::user_id::UserId,
}
impl DatabaseFixture {
    fn target(&self) -> Value {
        json!({"listingSourceId":self.source,"productListingId":self.id})
    }
    fn lifecycle(&self, version: u64) -> Value {
        let mut input = self.target();
        input["expectedVersion"] = json!(version);
        input
    }
    fn listing_calls(&self) -> Vec<(&'static str, Value)> {
        vec![
            (
                "listPartnerProductListings",
                json!({"listingSourceId":self.source}),
            ),
            ("getPartnerProductListing", self.target()),
            ("updatePartnerProductListing", self.lifecycle(1)),
            ("withdrawPartnerProductListing", self.lifecycle(1)),
            ("restorePartnerProductListing", self.lifecycle(1)),
        ]
    }
    async fn assert_listing_calls_forbidden(&self, context: &InvocationContext) {
        for (name, input) in self.listing_calls() {
            assert!(matches!(
                error(&self.registry, name, input, context.clone()).await,
                OperationError::Forbidden
            ));
        }
    }
}
async fn grant_source(
    pool: &sqlx::PgPool,
    user_id: user_core::user_id::UserId,
    party: uuid::Uuid,
    source: ListingSourceId,
) {
    let partnership = uuid::Uuid::now_v7();
    sqlx::query("INSERT INTO partnerships(partnership_id,party_id) VALUES($1,$2)")
        .bind(partnership)
        .bind(party)
        .execute(pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO partnership_members(partnership_id,user_id) VALUES($1,$2)")
        .bind(partnership)
        .bind(user_id.into_uuid())
        .execute(pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO partnership_listing_source_grants(partnership_id,listing_source_id) VALUES($1,$2)").bind(partnership).bind(source.into_uuid()).execute(pool).await.unwrap();
}

async fn fixture() -> DatabaseFixture {
    let pool = get_postgres_client().await;
    let context = context();
    let Principal::DelegatedUser { user_id, .. } = context.operation.principal else {
        panic!("delegated actor")
    };
    sqlx::query("INSERT INTO users(user_id,email,tier,role) VALUES($1,$2,'FREE','USER')")
        .bind(user_id.into_uuid())
        .bind(format!("{user_id}@example.test"))
        .execute(&pool)
        .await
        .unwrap();
    let (party, source) = seed_source(&pool).await;
    let (foreign_party, foreign) = seed_source(&pool).await;
    grant_source(&pool, user_id, party, source).await;
    let id = seed_listing(&pool, source).await;
    seed_listing(&pool, source).await;
    let foreign_id = seed_listing(&pool, foreign).await;
    let runtime = Runtime::from_pool(pool.clone());
    let services = runtime.partner_listing_management();
    assert!(Arc::ptr_eq(
        &services.get,
        &runtime.partner_listing_management().get
    ));
    let registry = Registry::new(operations(services.clone()).unwrap()).unwrap();
    DatabaseFixture {
        pool,
        registry,
        context,
        source,
        id,
        foreign_id,
        foreign_source: foreign,
        foreign_party,
        user_id,
    }
}

#[aura_integration_test(services = [SCHEMA], isolated = true)]
async fn should_keep_directory_continuations_and_listing_ids_within_the_authorized_source() {
    let f = fixture().await;
    let sources = call(
        &f.registry,
        "getMyListingSources",
        json!({"size":1}),
        f.context.clone(),
    )
    .await
    .unwrap();
    assert_eq!(json!(f.source), sources["items"][0]["listingSourceId"]);
    assert!(sources.get("next").is_none());
    let page = call(
        &f.registry,
        "listPartnerProductListings",
        json!({"listingSourceId":f.source,"size":1}),
        f.context.clone(),
    )
    .await
    .unwrap();
    assert!(page["searchAfter"].is_string());
    assert!(matches!(error(&f.registry,"listPartnerProductListings",json!({"listingSourceId":f.source,"size":1,"order":"asc","searchAfter":page["searchAfter"]}),f.context.clone()).await,OperationError::InvalidArguments(_)));
    for name in [
        "getPartnerProductListing",
        "updatePartnerProductListing",
        "withdrawPartnerProductListing",
        "restorePartnerProductListing",
    ] {
        let mut cross = json!({"listingSourceId":f.source,"productListingId":f.foreign_id});
        if name != "getPartnerProductListing" {
            cross["expectedVersion"] = json!(1);
        }
        assert!(matches!(
            error(&f.registry, name, cross, f.context.clone()).await,
            OperationError::NotFound
        ));
    }
    grant_source(&f.pool, f.user_id, f.foreign_party, f.foreign_source).await;
    assert!(matches!(
        error(
            &f.registry,
            "listPartnerProductListings",
            json!({"listingSourceId":f.foreign_source,"searchAfter":page["searchAfter"]}),
            f.context.clone()
        )
        .await,
        OperationError::InvalidArguments(_)
    ));
}

#[aura_integration_test(services = [SCHEMA], isolated = true)]
async fn should_preserve_authoritative_patches_version_conflicts_and_retained_lifecycle_facts() {
    let f = fixture().await;
    let detail = call(
        &f.registry,
        "getPartnerProductListing",
        f.target(),
        f.context.clone(),
    )
    .await
    .unwrap();
    assert_eq!(1, detail["expectedVersion"]);
    assert_eq!("Retained", detail["description"]["text"]);
    let mut patch = f.lifecycle(1);
    patch["price"] = json!({"type":"ON_REQUEST"});
    patch["images"] = json!([]);
    let changed = call(
        &f.registry,
        "updatePartnerProductListing",
        patch.clone(),
        f.context.clone(),
    )
    .await
    .unwrap();
    assert_eq!(2, changed["expectedVersion"]);
    assert_eq!("CHANGED", changed["outcome"]);
    assert!(matches!(
        error(
            &f.registry,
            "updatePartnerProductListing",
            patch,
            f.context.clone()
        )
        .await,
        OperationError::Conflict
    ));
    let detail = call(
        &f.registry,
        "getPartnerProductListing",
        f.target(),
        f.context.clone(),
    )
    .await
    .unwrap();
    assert_eq!("ON_REQUEST", detail["price"]["type"]);
    assert_eq!(json!([]), detail["images"]);
    assert_eq!(
        3,
        call(
            &f.registry,
            "withdrawPartnerProductListing",
            f.lifecycle(2),
            f.context.clone()
        )
        .await
        .unwrap()["expectedVersion"]
    );
    let retained = call(
        &f.registry,
        "getPartnerProductListing",
        f.target(),
        f.context.clone(),
    )
    .await
    .unwrap();
    assert_eq!("WITHDRAWN", retained["lifecycle"]);
    assert_eq!("Retained", retained["description"]["text"]);
    assert_eq!(
        4,
        call(
            &f.registry,
            "restorePartnerProductListing",
            f.lifecycle(3),
            f.context.clone()
        )
        .await
        .unwrap()["expectedVersion"]
    );
}

#[aura_integration_test(services = [SCHEMA], isolated = true)]
async fn should_recheck_capabilities_and_current_grants_after_tool_discovery() {
    let f = fixture().await;
    let (listed, _) = f.registry.page(None).unwrap();
    assert_eq!(6, listed.len());
    let mut unscoped = f.context.clone();
    unscoped.operation.principal = Principal::DelegatedUser {
        user_id: f.user_id,
        capabilities: BTreeSet::new(),
    };
    f.assert_listing_calls_forbidden(&unscoped).await;
    assert!(matches!(
        error(&f.registry, "getMyListingSources", json!({}), unscoped).await,
        OperationError::Forbidden
    ));
    sqlx::query("DELETE FROM partnership_listing_source_grants WHERE listing_source_id=$1")
        .bind(f.source.into_uuid())
        .execute(&f.pool)
        .await
        .unwrap();
    f.assert_listing_calls_forbidden(&f.context).await;
    assert_eq!(
        json!([]),
        call(
            &f.registry,
            "getMyListingSources",
            json!({}),
            f.context.clone()
        )
        .await
        .unwrap()["items"]
    );
}
