use application::transaction::{Transaction, UnitOfWork};
use indexmap::IndexSet;
use localization::Language;
use localization::Localized;
use money::Currency;
use money::{MonetaryAmount, Price};
use platform_postgres::SqlxUnitOfWork;
use product_listing_core::description::Description;
use product_listing_core::listing_availability::ListingAvailability;
use product_listing_core::product_listing::{
    NewProductListing, ProductListing, ProductListingAddress, ProductListingAuction,
    ProductListingPricing,
};
use product_listing_core::product_listing_id::ProductListingId;
use product_listing_core::product_listing_image::ProductListingImage;
use product_listing_core::prohibited_content::ProhibitedContent;
use product_listing_core::title::Title;
use product_listing_postgres::{
    SqlxProductListingEventStoreFactory, SqlxProductListingRepositoryFactory,
};
use product_listing_service::ports::{
    ProductListingEventStore, ProductListingEventStoreError, ProductListingEventStoreFactory,
    ProductListingRepository, ProductListingRepositoryFactory, stamp_product_listing_events,
};
use shop_core::shop_id::ShopId;
use shop_core::shop_name::ShopName;
use test_api::{IntegrationTestService, Postgres, aura_integration_test, get_postgres_client};
use time::OffsetDateTime;
use url::Url;

const BUSINESS_SCHEMA: Postgres = Postgres::new("migrations");

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_report_duplicate_event_in_product_listing_postgres() {
    let pool = get_postgres_client().await;
    let unit_of_work = SqlxUnitOfWork::new(pool.clone());
    let product_listings = SqlxProductListingRepositoryFactory::new();
    let events = SqlxProductListingEventStoreFactory::new();
    let shop_id = seed_shop(&pool, "product-listing-postgres-conflict-shop").await;
    let seller_id = seed_shop(&pool, "product-listing-postgres-conflict-seller").await;
    let product = sample_product("postgres-product-conflict", shop_id, seller_id);
    let event = first_stamped_event(&product);

    let mut tx = begin(&unit_of_work).await;
    match product_listings
        .in_transaction(&mut tx)
        .insert(&product, event.event_id)
        .await
    {
        Ok(_) => {}
        Err(error) => panic!("failed to insert product: {error:?}"),
    }
    match events.in_transaction(&mut tx).append(&event).await {
        Ok(_) => {}
        Err(error) => panic!("failed to append first event: {error:?}"),
    }
    commit(tx).await;

    let mut tx = begin(&unit_of_work).await;
    let duplicate_event = events.in_transaction(&mut tx).append(&event).await;
    assert!(matches!(
        duplicate_event,
        Err(ProductListingEventStoreError::ProductListingEventAlreadyExists)
    ));
}

fn first_stamped_event(
    product: &ProductListing,
) -> product_listing_service::ports::product_listing_event_store::ProductListingEvent {
    match stamp_product_listing_events(
        product.id(),
        OffsetDateTime::now_utc(),
        product.pending_event_payloads().to_vec(),
    )
    .into_iter()
    .next()
    {
        Some(event) => event,
        None => panic!("product is missing a pending event payload"),
    }
}

fn sample_product(slug: &str, shop_id: ShopId, seller_id: ShopId) -> ProductListing {
    let mut images = IndexSet::new();
    images.insert(ProductListingImage {
        url: url(&format!("https://example.com/{slug}.jpg")),
        prohibited_content: ProhibitedContent::None,
    });
    match ProductListing::create(NewProductListing {
        id: ProductListingId::new(),
        shop_id,
        seller_id,
        shop_listing_id: product_listing_core::shop_listing_id::ShopListingId::from(slug),
        address: ProductListingAddress::default(),
        title: Some(Localized::new(Language::En, Title::from(slug))),
        description: Some(Localized::new(
            Language::En,
            Description::from("Nice product"),
        )),
        pricing: ProductListingPricing {
            price: Some(Price::new(MonetaryAmount::from(1_200_u64), Currency::Eur)),
            price_estimate_min: None,
            price_estimate_max: None,
        },
        availability: Some(ListingAvailability::Available),
        url: url(&format!("https://example.com/{slug}")),
        images,
        auction: ProductListingAuction::default(),
    }) {
        Ok(product) => product,
        Err(error) => panic!("failed to create product: {error}"),
    }
}

async fn seed_shop(pool: &sqlx::PgPool, slug: &str) -> ShopId {
    let shop_id = ShopId::new();
    let result = sqlx::query(
        r#"
        INSERT INTO shops (shop_id, shop_slug_id, name, shop_type, partner_status, shop_domains)
        VALUES ($1, $2, $3, $4, $5, $6)
        "#,
    )
    .bind(uuid::Uuid::from(shop_id))
    .bind(slug)
    .bind(ShopName::from(slug).to_string())
    .bind("COMMERCIAL_DEALER")
    .bind("SCRAPED")
    .bind(Vec::<String>::from([format!("{slug}.example")]))
    .execute(pool)
    .await;

    if let Err(error) = result {
        panic!("failed to seed shop: {error}");
    }

    shop_id
}

fn url(value: &str) -> Url {
    match Url::parse(value) {
        Ok(url) => url,
        Err(error) => panic!("invalid test URL: {error}"),
    }
}

async fn begin(unit_of_work: &SqlxUnitOfWork) -> platform_postgres::SqlxTransaction {
    match unit_of_work.begin().await {
        Ok(tx) => tx,
        Err(error) => panic!("failed to begin transaction: {error}"),
    }
}

async fn commit(tx: platform_postgres::SqlxTransaction) {
    if let Err(error) = tx.commit().await {
        panic!("failed to commit transaction: {error}");
    }
}
