use common::currency::domain::Currency;
use common::language::domain::Language;
use common::localized::Localized;
use common::postgres::SqlxUnitOfWork;
use common::price::domain::{MonetaryAmount, Price};
use common::product_id::ProductId;
use common::product_state::domain::ProductState;
use common::shop_id::ShopId;
use common::shop_name::ShopName;
use common::transaction::{Transaction, UnitOfWork};
use indexmap::IndexSet;
use product_core::description::Description;
use product_core::product::{NewProduct, Product, ProductAddress, ProductAuction, ProductPricing};
use product_core::product_image::ProductImage;
use product_core::prohibited_content::ProhibitedContent;
use product_core::title::Title;
use product_postgres::{
    SqlxProductEventStoreFactory, SqlxProductRepositoryFactory, SqlxProductTranslationReaderFactory,
};
use product_service::ports::{
    ProductEventStore, ProductEventStoreFactory, ProductRepository, ProductRepositoryFactory,
    ProductTranslationReader, ProductTranslationReaderFactory,
};
use test_api::{IntegrationTestService, Postgres, aura_integration_test, get_postgres_client};
use url::Url;

const BUSINESS_SCHEMA: Postgres = Postgres::new("migrations");

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_read_mapped_title_and_description_translations_from_postgres() {
    let pool = get_postgres_client().await;
    let product_id = persist_product(&pool, "translation-reader-mapped").await;

    let translation_result = sqlx::query(
        r#"
        INSERT INTO product_translations (product_id, language, title, description)
        VALUES
            ($1, 'de', 'Übersetzter Titel', NULL),
            ($1, 'en', NULL, 'Translated description')
        "#,
    )
    .bind(uuid::Uuid::from(product_id))
    .execute(&pool)
    .await;
    if let Err(error) = translation_result {
        panic!("failed to seed product translations: {error}");
    }

    let translations = SqlxProductTranslationReaderFactory::new();
    let unit_of_work = SqlxUnitOfWork::new(pool);
    let mut tx = begin(&unit_of_work).await;
    let view = match translations
        .in_transaction(&mut tx)
        .find_for_product(product_id)
        .await
    {
        Ok(view) => view,
        Err(error) => panic!("failed to read product translations: {error:?}"),
    };
    commit(tx).await;

    assert_eq!(view.product_id, product_id);
    assert_eq!(view.titles[&Language::De].as_ref(), "Übersetzter Titel");
    assert_eq!(
        view.descriptions[&Language::En].as_ref(),
        "Translated description"
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_return_empty_translation_maps_when_product_has_no_translations() {
    let pool = get_postgres_client().await;
    let product_id = persist_product(&pool, "translation-reader-empty").await;
    let translations = SqlxProductTranslationReaderFactory::new();
    let unit_of_work = SqlxUnitOfWork::new(pool);

    let mut tx = begin(&unit_of_work).await;
    let view = match translations
        .in_transaction(&mut tx)
        .find_for_product(product_id)
        .await
    {
        Ok(view) => view,
        Err(error) => panic!("failed to read product translations: {error:?}"),
    };
    commit(tx).await;

    assert_eq!(view.product_id, product_id);
    assert!(view.titles.is_empty());
    assert!(view.descriptions.is_empty());
}

async fn persist_product(pool: &sqlx::PgPool, slug: &str) -> ProductId {
    let unit_of_work = SqlxUnitOfWork::new(pool.clone());
    let products = SqlxProductRepositoryFactory::new();
    let events = SqlxProductEventStoreFactory::new();
    let shop_id = seed_shop(pool, &format!("{slug}-shop")).await;
    let seller_id = seed_shop(pool, &format!("{slug}-seller")).await;
    let product = sample_product(slug, shop_id, seller_id);
    let event = product.pending_events()[0].clone();

    let mut tx = begin(&unit_of_work).await;
    match products
        .in_transaction(&mut tx)
        .insert(&product, event.event_id)
        .await
    {
        Ok(_) => {}
        Err(error) => panic!("failed to persist product: {error:?}"),
    }
    match events.in_transaction(&mut tx).append(&event).await {
        Ok(_) => {}
        Err(error) => panic!("failed to persist product event: {error:?}"),
    }
    commit(tx).await;

    product.id()
}

fn sample_product(slug: &str, shop_id: ShopId, seller_id: ShopId) -> Product {
    let mut images = IndexSet::new();
    images.insert(ProductImage {
        url: url(&format!("https://example.com/{slug}.jpg")),
        prohibited_content: ProhibitedContent::None,
    });

    match Product::create(NewProduct {
        id: ProductId::new(),
        shop_id,
        seller_id,
        shops_product_id: common::shops_product_id::ShopsProductId::from(slug),
        address: ProductAddress::default(),
        title: Some(Localized::new(Language::En, Title::from(slug))),
        description: Some(Localized::new(
            Language::En,
            Description::from("Nice product"),
        )),
        pricing: ProductPricing {
            price: Some(Price::new(MonetaryAmount::from(1_200_u64), Currency::Eur)),
            price_estimate_min: None,
            price_estimate_max: None,
            fx_rate_id: None,
        },
        state: ProductState::Listed,
        url: url(&format!("https://example.com/{slug}")),
        images,
        auction: ProductAuction::default(),
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

async fn begin(unit_of_work: &SqlxUnitOfWork) -> common::postgres::SqlxTransaction {
    match unit_of_work.begin().await {
        Ok(tx) => tx,
        Err(error) => panic!("failed to begin transaction: {error}"),
    }
}

async fn commit(tx: common::postgres::SqlxTransaction) {
    if let Err(error) = tx.commit().await {
        panic!("failed to commit transaction: {error}");
    }
}
