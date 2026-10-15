//! Origin response enrichment remains current while indexed membership catches up.
use application::{
    operation_context::{OperationContext, Principal},
    transaction::{Transaction, UnitOfWork},
};
use platform_postgres::SqlxUnitOfWork;
use product_listing_core::{
    product_listing_id::ProductListingId, product_listing_search::ProductListingSearch,
};
use product_listing_service::{ports::*, shipping::ShippingEnrichment, use_cases::*};
use serde_json::{Value, json};
use shipping_postgres::{
    assessment::SqlxAssessmentReaderFactory, projection::SqlxShippingProjectionStoreFactory,
};
use shipping_service::{assessment::AssessDeliveryHandler, projection::*};
use std::sync::Arc;
use test_api::{
    IntegrationTestService, OpenSearch, Postgres, aura_integration_test, get_opensearch_client,
    get_postgres_client, refresh_index,
};
use user_service::address_book::{AddressBookCommands, AddressBookHandler};
#[path = "../../shipping-postgres/tests/support/mod.rs"]
mod support;
use support::*;
const PG: Postgres = Postgres::new("migrations");
struct NoEmbeddings;
#[async_trait::async_trait]
impl embedding::EmbeddingGenerator for NoEmbeddings {
    async fn embed_product(
        &self,
        _: &embedding::EmbeddingText,
        _: Option<&embedding::EmbeddingText>,
        _: Option<&embedding::EmbeddingImageUrl>,
    ) -> Result<embedding::EmbeddingVector, embedding::EmbeddingError> {
        panic!("no provider calls")
    }
    async fn embed_search_query(
        &self,
        _: &embedding::EmbeddingText,
    ) -> Result<embedding::EmbeddingVector, embedding::EmbeddingError> {
        panic!("no provider calls")
    }
}
fn context(principal: Principal) -> OperationContext {
    OperationContext {
        principal,
        request_id: "shipping-origin".into(),
        correlation_id: "shipping-origin".into(),
    }
}
async fn drain(pool: &sqlx::PgPool) {
    for _ in 0..50 {
        ProjectListingShippingHandler::new(
            SqlxUnitOfWork::new(pool.clone()),
            SqlxShippingProjectionStoreFactory,
        )
        .execute(None, 100)
        .await
        .unwrap();
        let pending: bool =
            sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM listing_shipping_work WHERE pending)")
                .fetch_one(pool)
                .await
                .unwrap();
        if !pending {
            return;
        }
    }
    panic!("shipping did not drain");
}
async fn project(pool: &sqlx::PgPool, ids: &[ProductListingId]) {
    let mut tx = SqlxUnitOfWork::new(pool.clone()).begin().await.unwrap();
    let refs=sqlx::query_as::<_,(uuid::Uuid,uuid::Uuid)>("SELECT product_listing_id,current_event_id FROM product_listings WHERE product_listing_id=ANY($1)").bind(ids.iter().map(|id|id.into_uuid()).collect::<Vec<_>>()).fetch_all(tx.connection()).await.unwrap().into_iter().map(|(id,event)|ProductListingSearchFilterMatchSourceRef{product_listing_id:id.try_into().unwrap(),event_id:event.try_into().unwrap()}).collect::<Vec<_>>();
    let sources = product_listing_postgres::SqlxProductListingSearchFilterMatchSourceReaderFactory
        .in_transaction(&mut tx)
        .find_sources(&refs)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let writer = product_listing_opensearch::OpenSearchProductListingSearchProjection::new(
        get_opensearch_client().await.clone(),
    );
    for source in sources.values() {
        writer.upsert(source, None).await.unwrap();
    }
    refresh_index("product-listings").await;
}
async fn indexed_old_rate_count() -> u64 {
    let body:Value=get_opensearch_client().await.search(opensearch::SearchParts::Index(&["product-listings"])).body(json!({"query":product_listing_opensearch::shipping_query::country_offer_query(geo::CountryCode::DEU,Some((money::Currency::Eur,8000)))})).send().await.unwrap().error_for_status_code().unwrap().json().await.unwrap();
    body["hits"]["total"]["value"].as_u64().unwrap()
}

#[aura_integration_test(services=[PG])]
async fn complete_projection_waits_for_shipping_writer_before_reading_summary_and_fence() {
    let pool = get_postgres_client().await;
    let (_, _, ids) = seed(&pool).await;
    drain(&pool).await;
    let id = ids[0];
    let mut writer = pool.begin().await.unwrap();
    let version: i64 = sqlx::query_scalar(
        "UPDATE product_listings SET projection_version=projection_version+1 WHERE product_listing_id=$1 RETURNING projection_version",
    )
    .bind(id.into_uuid())
    .fetch_one(&mut *writer)
    .await
    .unwrap();
    sqlx::query("UPDATE product_listing_shipping SET summary=jsonb_set(summary,'{country_offers,0,per_item_price,amount}','9500'),shipping_version=$2 WHERE product_listing_id=$1")
        .bind(id.into_uuid()).bind(version).execute(&mut *writer).await.unwrap();

    let (started, waiting) = tokio::sync::oneshot::channel();
    let reader_pool = pool.clone();
    let reader = tokio::spawn(async move {
        let mut tx = SqlxUnitOfWork::new(reader_pool).begin().await.unwrap();
        let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(tx.connection())
            .await
            .unwrap();
        let event: uuid::Uuid = sqlx::query_scalar(
            "SELECT current_event_id FROM product_listings WHERE product_listing_id=$1",
        )
        .bind(id.into_uuid())
        .fetch_one(tx.connection())
        .await
        .unwrap();
        started.send(pid).unwrap();
        let reference = ProductListingSearchFilterMatchSourceRef {
            product_listing_id: id,
            event_id: event.try_into().unwrap(),
        };
        let mut sources =
            product_listing_postgres::SqlxProductListingSearchFilterMatchSourceReaderFactory
                .in_transaction(&mut tx)
                .find_sources(&[reference])
                .await
                .unwrap();
        tx.commit().await.unwrap();
        sources.remove(&reference).unwrap()
    });
    let pid = waiting.await.unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            let blocked: bool = sqlx::query_scalar("SELECT cardinality(pg_blocking_pids($1))>0")
                .bind(pid)
                .fetch_one(&pool)
                .await
                .unwrap();
            if blocked {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("projection reader must wait for the writer's listing lock");
    writer.commit().await.unwrap();
    let source = tokio::time::timeout(std::time::Duration::from_secs(5), reader)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(version, source.projection_version);
    assert_eq!(
        9500,
        u64::from(
            source.shipping.country_offers[0]
                .per_item_price
                .unwrap()
                .monetary_amount
        )
    );
}

#[aura_integration_test(services=[PG,OpenSearch()])]
async fn search_keeps_indexed_membership_and_order_but_returns_current_postgres_shipping() {
    let pool = get_postgres_client().await;
    let (_, profile, ids) = seed(&pool).await;
    // Give origin presentation an approved title and immutable valuation snapshot.
    sqlx::query("UPDATE product_listings SET title_text='Gallery object',title_language='en'")
        .execute(&pool)
        .await
        .unwrap();
    let fx = fxrate_core::FxRateId::new();
    sqlx::query("INSERT INTO fx_rates(fx_rate_id,captured_at,source,source_event_id)VALUES($1,now(),'fxratesapi',$2)").bind(fx.into_uuid()).bind(fx.to_string()).execute(&pool).await.unwrap();
    use strum::IntoEnumIterator;
    for currency in money::Currency::iter() {
        sqlx::query(
            "INSERT INTO fx_rate_quotes(fx_rate_id,currency,units_per_eur)VALUES($1,$2,1000000)",
        )
        .bind(fx.into_uuid())
        .bind(currency.as_str())
        .execute(&pool)
        .await
        .unwrap();
    }
    let user = user_core::user_id::UserId::new();
    sqlx::query("INSERT INTO users(user_id,email,tier,role)VALUES($1,$2,'FREE','USER')")
        .bind(user.into_uuid())
        .bind(format!("{user}@example.test"))
        .execute(&pool)
        .await
        .unwrap();
    let book = Arc::new(AddressBookHandler::new(
        SqlxUnitOfWork::new(pool.clone()),
        user_postgres::SqlxAddressBookFactory,
        geo_postgres::SqlxResolutionRepositoryFactory,
    ));
    book.set_destination(
        &context(Principal::User(user)),
        user_core::user_address::BrowsingDestination::CountryPostal {
            country: geo::CountryCode::DEU,
            postal_code: None,
        },
        1,
    )
    .await
    .unwrap();
    let shipping = ShippingEnrichment::new(Arc::new(AssessDeliveryHandler::new(
        SqlxUnitOfWork::new(pool.clone()),
        SqlxAssessmentReaderFactory,
        book,
    )));
    let handler = SearchProductListingsHandler::new(
        product_listing_opensearch::OpenSearchProductListingSearchReader::new(
            get_opensearch_client().await.clone(),
        ),
        fxrate_postgres::SqlxFxRateSnapshotReader::new(pool.clone()),
        NoEmbeddings,
        product_listing_postgres::SqlxListingSourceSummaryReader::new(pool.clone()),
        product_listing_postgres::SqlxProductListingUserStateReader::new(pool.clone()),
        product_listing_postgres::SqlxProductListingContentAssessmentReader::new(pool.clone()),
        shipping,
    );
    let request = || SearchProductListingsRequest {
        search: ProductListingSearch::default(),
        sort: None,
        cursor: None,
    };
    drain(&pool).await;
    project(&pool, &ids).await;
    let before = handler
        .execute(&context(Principal::User(user)), request())
        .await
        .unwrap();
    assert_eq!(Some(2), before.total);
    assert_eq!(1, indexed_old_rate_count().await);
    sqlx::query("UPDATE shipping_profiles SET terms=$2,management_revision=2,terms_revision=2 WHERE shipping_profile_id=$1").bind(profile.into_uuid()).bind(shipping_json::encode::<References>(&shipping_core::fixtures::gallery_terms(9500)).unwrap()).execute(&pool).await.unwrap();
    let current = handler
        .execute(&context(Principal::User(user)), request())
        .await
        .unwrap();
    assert_eq!(before.total, current.total);
    assert_eq!(
        before
            .items
            .iter()
            .map(|i| i.item.product_listing_id)
            .collect::<Vec<_>>(),
        current
            .items
            .iter()
            .map(|i| i.item.product_listing_id)
            .collect::<Vec<_>>()
    );
    let painting = current
        .items
        .iter()
        .find(|i| i.item.product_listing_id == ids[0])
        .unwrap();
    assert_eq!(
        &shipping_core::fixtures::gallery_terms(9500),
        painting.item.shipping.as_ref().unwrap().effective.terms()
    );
    assert_eq!(
        Some(shipping_core::terms::ShippingEligibility::Offered),
        painting
            .user_state
            .as_ref()
            .unwrap()
            .shipping
            .as_ref()
            .unwrap()
            .assessment
            .eligibility
    );
    let personalized = &painting
        .user_state
        .as_ref()
        .unwrap()
        .shipping
        .as_ref()
        .unwrap()
        .assessment;
    assert_eq!(
        shipping_core::pricing::ShippingPrice::Fixed(money::Price::new(
            9500_u64.into(),
            money::Currency::Eur
        )),
        personalized.services[0].service.rate().unwrap().price()
    );
    assert!(personalized.services[0].charge.is_none());
    assert_eq!(1, indexed_old_rate_count().await);
    drain(&pool).await;
    project(&pool, &ids).await;
    assert_eq!(0, indexed_old_rate_count().await);
    let caught_up = handler
        .execute(&context(Principal::User(user)), request())
        .await
        .unwrap();
    assert_eq!(current.total, caught_up.total);
    assert_eq!(
        current
            .items
            .iter()
            .map(|i| i.item.product_listing_id)
            .collect::<Vec<_>>(),
        caught_up
            .items
            .iter()
            .map(|i| i.item.product_listing_id)
            .collect::<Vec<_>>()
    );
}
