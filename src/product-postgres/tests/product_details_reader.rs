use common::currency::domain::Currency;
use common::language::domain::Language;
use common::localized::Localized;
use common::postgres::SqlxUnitOfWork;
use common::price::domain::{MonetaryAmount, Price};
use common::product_id::ProductId;
use common::product_state::domain::ProductState;
use common::shop_id::ShopId;
use common::shop_name::ShopName;
use common::shop_slug_id::ShopSlugId;
use common::transaction::{Transaction, UnitOfWork};
use indexmap::IndexSet;
use product_core::description::Description;
use product_core::product::{NewProduct, Product, ProductAddress, ProductAuction, ProductPricing};
use product_core::product_image::ProductImage;
use product_core::prohibited_content::ProhibitedContent;
use product_core::title::Title;
use product_postgres::{
    SqlxProductDetailsReaderFactory, SqlxProductEventStoreFactory, SqlxProductRepositoryFactory,
};
use product_service::ports::{
    ProductDetailsReader, ProductDetailsReaderFactory, ProductEventStore, ProductEventStoreFactory,
    ProductRepository, ProductRepositoryFactory,
};
use product_service::use_cases::queries::get_product::{
    GetProductRequest, ProductDetailsView, ProductLookup,
};
use test_api::{IntegrationTestService, Postgres, aura_integration_test, get_postgres_client};
use url::Url;

const BUSINESS_SCHEMA: Postgres = Postgres::new("migrations");

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_select_requested_translations_independently_and_preserve_original_text_for_id_lookup()
 {
    let pool = get_postgres_client().await;
    let product = persist_product(
        &pool,
        "details-requested",
        Some(Localized::new(Language::En, Title::from("Original title"))),
        Some(Localized::new(
            Language::En,
            Description::from("Original description"),
        )),
    )
    .await;

    insert_translation(&pool, product.id(), "de", Some("Deutscher Titel"), None).await;
    insert_translation(
        &pool,
        product.id(),
        "en",
        None,
        Some("Translated description"),
    )
    .await;

    let view = find_details(
        &pool,
        GetProductRequest {
            lookup: ProductLookup::ById(product.id()),
            language: Language::De,
        },
    )
    .await;

    assert_localized_title(view.product_title.as_ref(), Language::En, "Original title");
    assert_localized_description(
        view.product_description.as_ref(),
        Language::En,
        "Original description",
    );
    assert_localized_title(view.title.as_ref(), Language::De, "Deutscher Titel");
    assert_localized_description(
        view.description.as_ref(),
        Language::En,
        "Translated description",
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_fall_back_to_de_then_deterministic_remaining_translation_for_slug_lookup() {
    let pool = get_postgres_client().await;
    let shop_slug = "details-fallback-shop";
    let product = persist_product_with_shop_slug(&pool, "details-fallback", shop_slug).await;

    insert_translation(&pool, product.id(), "de", Some("Deutscher Titel"), None).await;
    insert_translation(
        &pool,
        product.id(),
        "fr",
        Some("Titre français"),
        Some("Description française"),
    )
    .await;
    insert_translation(
        &pool,
        product.id(),
        "es",
        Some("Título español"),
        Some("Descripción española"),
    )
    .await;

    let view = find_details(
        &pool,
        GetProductRequest {
            lookup: ProductLookup::BySlug {
                shop_slug_id: ShopSlugId::from(shop_slug),
                product_slug_id: product.slug_id().clone(),
            },
            language: Language::It,
        },
    )
    .await;

    assert!(view.product_title.is_none());
    assert!(view.product_description.is_none());
    assert_localized_title(view.title.as_ref(), Language::De, "Deutscher Titel");
    assert_localized_description(
        view.description.as_ref(),
        Language::Es,
        "Descripción española",
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_return_original_english_text_when_no_translation_has_selected_text() {
    let pool = get_postgres_client().await;
    let product = persist_product(
        &pool,
        "details-original-fallback",
        Some(Localized::new(Language::En, Title::from("Original title"))),
        Some(Localized::new(
            Language::En,
            Description::from("Original description"),
        )),
    )
    .await;

    insert_translation(&pool, product.id(), "de", Some("Deutscher Titel"), None).await;

    let view = find_details(
        &pool,
        GetProductRequest {
            lookup: ProductLookup::ById(product.id()),
            language: Language::Fr,
        },
    )
    .await;

    assert_localized_title(view.title.as_ref(), Language::En, "Original title");
    assert_localized_description(
        view.description.as_ref(),
        Language::En,
        "Original description",
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_return_no_selected_text_when_product_has_no_stored_or_translated_text() {
    let pool = get_postgres_client().await;
    let product = persist_product(&pool, "details-no-text", None, None).await;

    let view = find_details(
        &pool,
        GetProductRequest {
            lookup: ProductLookup::ById(product.id()),
            language: Language::En,
        },
    )
    .await;

    assert!(view.product_title.is_none());
    assert!(view.product_description.is_none());
    assert!(view.title.is_none());
    assert!(view.description.is_none());
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_return_none_when_product_is_missing() {
    let pool = get_postgres_client().await;
    let unit_of_work = SqlxUnitOfWork::new(pool.clone());
    let details = SqlxProductDetailsReaderFactory::new();
    let mut tx = begin(&unit_of_work).await;
    let result = details
        .in_transaction(&mut tx)
        .find_details(&GetProductRequest {
            lookup: ProductLookup::ById(ProductId::new()),
            language: Language::En,
        })
        .await;
    commit(tx).await;

    match result {
        Ok(None) => {}
        Ok(Some(_)) => panic!("found missing product details"),
        Err(error) => panic!("failed to query missing product details: {error:?}"),
    }
}

async fn find_details(pool: &sqlx::PgPool, request: GetProductRequest) -> ProductDetailsView {
    let unit_of_work = SqlxUnitOfWork::new(pool.clone());
    let details = SqlxProductDetailsReaderFactory::new();
    let mut tx = begin(&unit_of_work).await;
    let result = details.in_transaction(&mut tx).find_details(&request).await;
    commit(tx).await;

    match result {
        Ok(Some(view)) => view,
        Ok(None) => panic!("missing product details"),
        Err(error) => panic!("failed to read product details: {error:?}"),
    }
}

async fn persist_product(
    pool: &sqlx::PgPool,
    slug: &str,
    title: Option<Localized<Language, Title>>,
    description: Option<Localized<Language, Description>>,
) -> Product {
    let shop_id = seed_shop(pool, &format!("{slug}-shop")).await;
    let seller_id = seed_shop(pool, &format!("{slug}-seller")).await;
    persist_product_for_shops(pool, slug, shop_id, seller_id, title, description).await
}

async fn persist_product_with_shop_slug(
    pool: &sqlx::PgPool,
    slug: &str,
    shop_slug: &str,
) -> Product {
    let shop_id = seed_shop(pool, shop_slug).await;
    let seller_id = seed_shop(pool, &format!("{slug}-seller")).await;
    persist_product_for_shops(pool, slug, shop_id, seller_id, None, None).await
}

async fn persist_product_for_shops(
    pool: &sqlx::PgPool,
    slug: &str,
    shop_id: ShopId,
    seller_id: ShopId,
    title: Option<Localized<Language, Title>>,
    description: Option<Localized<Language, Description>>,
) -> Product {
    let unit_of_work = SqlxUnitOfWork::new(pool.clone());
    let products = SqlxProductRepositoryFactory::new();
    let events = SqlxProductEventStoreFactory::new();
    let product = sample_product(slug, shop_id, seller_id, title, description);
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

    product
}

async fn insert_translation(
    pool: &sqlx::PgPool,
    product_id: ProductId,
    language: &str,
    title: Option<&str>,
    description: Option<&str>,
) {
    let result = sqlx::query(
        r#"
        INSERT INTO product_translations (product_id, language, title, description)
        VALUES ($1, $2, $3, $4)
        "#,
    )
    .bind(uuid::Uuid::from(product_id))
    .bind(language)
    .bind(title)
    .bind(description)
    .execute(pool)
    .await;

    if let Err(error) = result {
        panic!("failed to seed product translation: {error}");
    }
}

fn sample_product(
    slug: &str,
    shop_id: ShopId,
    seller_id: ShopId,
    title: Option<Localized<Language, Title>>,
    description: Option<Localized<Language, Description>>,
) -> Product {
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
        title,
        description,
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

fn assert_localized_title(
    value: Option<&Localized<Language, Title>>,
    language: Language,
    text: &str,
) {
    match value {
        Some(value) => {
            assert_eq!(value.localization, language);
            assert_eq!(value.payload.as_ref(), text);
        }
        None => panic!("missing title"),
    }
}

fn assert_localized_description(
    value: Option<&Localized<Language, Description>>,
    language: Language,
    text: &str,
) {
    match value {
        Some(value) => {
            assert_eq!(value.localization, language);
            assert_eq!(value.payload.as_ref(), text);
        }
        None => panic!("missing description"),
    }
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
