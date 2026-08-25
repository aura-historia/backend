use application::pagination::{Cursor, CursoredResult};
use application::transaction::{Transaction, UnitOfWork};
use indexmap::IndexSet;
use localization::Language;
use localization::Localized;
use money::Currency;
use money::{MonetaryAmount, Price};
use platform_postgres::SqlxUnitOfWork;
use product_listing_core::description::Description;
use product_listing_core::product_listing::{
    NewProductListing, ProductListing, ProductListingAddress, ProductListingAuction,
    ProductListingPricing,
};
use product_listing_core::product_listing_id::ProductListingId;
use product_listing_core::product_listing_image::ProductListingImage;
use product_listing_core::product_state::ProductState;
use product_listing_core::prohibited_content::ProhibitedContent;
use product_listing_core::title::Title;
use product_listing_postgres::{
    SqlxProductListingEventStoreFactory, SqlxProductListingRepositoryFactory,
    SqlxProductListingWatchlistDetailsReaderFactory,
};
use product_listing_service::ports::PersonalizedProductListingDetailsReadModel;
use product_listing_service::ports::{
    ProductListingEventStore, ProductListingEventStoreFactory, ProductListingRepository,
    ProductListingRepositoryFactory, ProductListingWatchlistDetailsCursor,
    ProductListingWatchlistDetailsReadError, ProductListingWatchlistDetailsReader,
    ProductListingWatchlistDetailsReaderFactory, ProductListingWatchlistDetailsRequest,
};
use shop_core::shop_id::ShopId;
use shop_core::shop_name::ShopName;
use test_api::{IntegrationTestService, Postgres, aura_integration_test, get_postgres_client};
use time::{Duration, OffsetDateTime};
use url::Url;
use user_core::user_id::UserId;

const BUSINESS_SCHEMA: Postgres = Postgres::new("migrations");

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_join_watchlisted_product_localization_and_user_state() {
    let pool = get_postgres_client().await;
    let product = persist_product(
        &pool,
        "watchlist-joined",
        Some(Localized::new(Language::En, Title::from("Original title"))),
        Some(Localized::new(
            Language::En,
            Description::from("Original description"),
        )),
    )
    .await;
    let user_id = seed_user(&pool, "FREE", false).await;

    insert_translation(
        &pool,
        product.id(),
        "de",
        Some("Deutscher Titel"),
        Some("Deutsche Beschreibung"),
    )
    .await;
    insert_watchlist(
        &pool,
        user_id,
        product.id(),
        false,
        OffsetDateTime::UNIX_EPOCH,
    )
    .await;

    let products = find_for_user(&pool, user_id, Language::De).await;
    let [view] = products.as_slice() else {
        panic!("expected one watchlisted product");
    };

    assert_eq!(view.item.product_id, product.id());
    assert_eq!(view.item.shop_listing_id.as_ref(), "watchlist-joined");
    assert_eq!(view.item.shop_name.as_ref(), "watchlist-joined-shop");
    assert_eq!(view.item.seller_name.as_ref(), "watchlist-joined-seller");
    assert!(view.item.sale_valuation.is_none());
    assert_localized_title(
        view.item.product_title.as_ref(),
        Language::En,
        "Original title",
    );
    assert_localized_description(
        view.item.product_description.as_ref(),
        Language::En,
        "Original description",
    );
    assert_localized_title(view.item.title.as_ref(), Language::De, "Deutscher Titel");
    assert_localized_description(
        view.item.description.as_ref(),
        Language::De,
        "Deutsche Beschreibung",
    );

    let Some(user_state) = view.user_state.as_ref() else {
        panic!("missing product user state");
    };
    assert!(user_state.watchlist.watching);
    assert!(!user_state.watchlist.notifications);
    assert!(user_state.prohibited_content.consent);
    assert!(!user_state.search_filter.matched);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_order_watchlisted_products_by_created_desc_then_product_id_asc() {
    let pool = get_postgres_client().await;
    let user_id = seed_user(&pool, "FREE", false).await;
    let first_tied = persist_product(&pool, "watchlist-order-first", None, None).await;
    let second_tied = persist_product(&pool, "watchlist-order-second", None, None).await;
    let newest = persist_product(&pool, "watchlist-order-newest", None, None).await;
    let tied_created = OffsetDateTime::UNIX_EPOCH + Duration::days(1);

    insert_watchlist(&pool, user_id, first_tied.id(), true, tied_created).await;
    insert_watchlist(&pool, user_id, second_tied.id(), true, tied_created).await;
    insert_watchlist(
        &pool,
        user_id,
        newest.id(),
        true,
        tied_created + Duration::seconds(1),
    )
    .await;

    let products = find_for_user(&pool, user_id, Language::En).await;
    let mut tied_ids = [first_tied.id(), second_tied.id()];
    tied_ids.sort_by_key(|product_id| uuid::Uuid::from(*product_id));
    let product_ids = products
        .into_iter()
        .map(|product| product.item.product_id)
        .collect::<Vec<_>>();

    assert_eq!(product_ids, vec![newest.id(), tied_ids[0], tied_ids[1]]);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_page_watchlisted_products_by_created_desc_then_product_id_asc() {
    let pool = get_postgres_client().await;
    let user_id = seed_user(&pool, "FREE", false).await;
    let first_tied = persist_product(&pool, "watchlist-page-first", None, None).await;
    let second_tied = persist_product(&pool, "watchlist-page-second", None, None).await;
    let third_tied = persist_product(&pool, "watchlist-page-third", None, None).await;
    let newest = persist_product(&pool, "watchlist-page-newest", None, None).await;
    let tied_created = OffsetDateTime::UNIX_EPOCH + Duration::days(1);

    for product_id in [first_tied.id(), second_tied.id(), third_tied.id()] {
        insert_watchlist(&pool, user_id, product_id, true, tied_created).await;
    }
    insert_watchlist(
        &pool,
        user_id,
        newest.id(),
        true,
        tied_created + Duration::seconds(1),
    )
    .await;

    let mut tied_ids = [first_tied.id(), second_tied.id(), third_tied.id()];
    tied_ids.sort_by_key(|product_id| uuid::Uuid::from(*product_id));
    let first_page = find_for_user_page(
        &pool,
        &ProductListingWatchlistDetailsRequest {
            user_id,
            language: Language::En,
            cursor: Cursor {
                size: 2,
                search_after: None,
            },
        },
    )
    .await;
    let expected_cursor = ProductListingWatchlistDetailsCursor {
        watchlist_created: tied_created,
        product_id: tied_ids[0],
    };

    assert_eq!(
        first_page
            .items
            .iter()
            .map(|product| product.item.product_id)
            .collect::<Vec<_>>(),
        vec![newest.id(), tied_ids[0]]
    );
    assert_eq!(first_page.cursor.search_after, Some(expected_cursor));

    let second_page = find_for_user_page(
        &pool,
        &ProductListingWatchlistDetailsRequest {
            user_id,
            language: Language::En,
            cursor: Cursor {
                size: 2,
                search_after: first_page.cursor.search_after,
            },
        },
    )
    .await;

    assert_eq!(
        second_page
            .items
            .iter()
            .map(|product| product.item.product_id)
            .collect::<Vec<_>>(),
        vec![tied_ids[1], tied_ids[2]]
    );
    assert_eq!(second_page.cursor.search_after, None);
}

async fn find_for_user(
    pool: &sqlx::PgPool,
    user_id: UserId,
    language: Language,
) -> Vec<PersonalizedProductListingDetailsReadModel> {
    match find_for_user_result(pool, user_id, language).await {
        Ok(products) => products,
        Err(error) => panic!("failed to read product watchlist details: {error:?}"),
    }
}

async fn find_for_user_result(
    pool: &sqlx::PgPool,
    user_id: UserId,
    language: Language,
) -> Result<Vec<PersonalizedProductListingDetailsReadModel>, ProductListingWatchlistDetailsReadError>
{
    find_for_user_page_result(
        pool,
        &ProductListingWatchlistDetailsRequest {
            user_id,
            language,
            cursor: Cursor {
                size: 100,
                search_after: None,
            },
        },
    )
    .await
    .map(|page| page.items)
}

async fn find_for_user_page(
    pool: &sqlx::PgPool,
    request: &ProductListingWatchlistDetailsRequest,
) -> CursoredResult<PersonalizedProductListingDetailsReadModel, ProductListingWatchlistDetailsCursor>
{
    match find_for_user_page_result(pool, request).await {
        Ok(page) => page,
        Err(error) => panic!("failed to read product watchlist details: {error:?}"),
    }
}

async fn find_for_user_page_result(
    pool: &sqlx::PgPool,
    request: &ProductListingWatchlistDetailsRequest,
) -> Result<
    CursoredResult<
        PersonalizedProductListingDetailsReadModel,
        ProductListingWatchlistDetailsCursor,
    >,
    ProductListingWatchlistDetailsReadError,
> {
    let unit_of_work = SqlxUnitOfWork::new(pool.clone());
    let details = SqlxProductListingWatchlistDetailsReaderFactory::new();
    let mut tx = begin(&unit_of_work).await;
    let result = details.in_transaction(&mut tx).find_for_user(request).await;
    commit(tx).await;
    result
}

async fn persist_product(
    pool: &sqlx::PgPool,
    slug: &str,
    title: Option<Localized<Language, Title>>,
    description: Option<Localized<Language, Description>>,
) -> ProductListing {
    let shop_id = seed_shop(pool, &format!("{slug}-shop")).await;
    let seller_id = seed_shop(pool, &format!("{slug}-seller")).await;
    let unit_of_work = SqlxUnitOfWork::new(pool.clone());
    let products = SqlxProductListingRepositoryFactory::new();
    let events = SqlxProductListingEventStoreFactory::new();
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
    product_id: ProductListingId,
    language: &str,
    title: Option<&str>,
    description: Option<&str>,
) {
    let result = sqlx::query(
        r#"
        INSERT INTO product_translations (product_id, source_event_id, language, title, description)
        SELECT product_id, event_id, $2, $3, $4
        FROM products
        WHERE product_id = $1
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

async fn seed_user(pool: &sqlx::PgPool, tier: &str, prohibited_content_consent: bool) -> UserId {
    let user_id = UserId::new();
    let result = sqlx::query(
        r#"
        INSERT INTO users (user_id, email, prohibited_content_consent, tier, role)
        VALUES ($1, $2, $3, $4, $5)
        "#,
    )
    .bind(uuid::Uuid::from(user_id))
    .bind(format!("{user_id}@example.test"))
    .bind(prohibited_content_consent)
    .bind(tier)
    .bind("USER")
    .execute(pool)
    .await;

    if let Err(error) = result {
        panic!("failed to seed user: {error}");
    }

    user_id
}

async fn insert_watchlist(
    pool: &sqlx::PgPool,
    user_id: UserId,
    product_id: ProductListingId,
    notifications: bool,
    created: OffsetDateTime,
) {
    let result = sqlx::query(
        r#"
        INSERT INTO product_watchlist (user_id, product_id, notifications, state, active_since, notifications_enabled_since, created, updated)
        VALUES ($1, $2, $3, $4, CASE WHEN $4 = 'ACTIVE' THEN $5 ELSE NULL END, CASE WHEN $3 THEN $5 ELSE NULL END, $5, $5)
        "#,
    )
    .bind(uuid::Uuid::from(user_id))
    .bind(uuid::Uuid::from(product_id))
    .bind(notifications)
    .bind("ACTIVE")
    .bind(created)
    .execute(pool)
    .await;

    if let Err(error) = result {
        panic!("failed to seed product watchlist: {error}");
    }
}

fn sample_product(
    slug: &str,
    shop_id: ShopId,
    seller_id: ShopId,
    title: Option<Localized<Language, Title>>,
    description: Option<Localized<Language, Description>>,
) -> ProductListing {
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
        title,
        description,
        pricing: ProductListingPricing {
            price: Some(Price::new(MonetaryAmount::from(1_200_u64), Currency::Eur)),
            price_estimate_min: None,
            price_estimate_max: None,
        },
        sale_valuation: None,
        state: ProductState::Listed,
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
