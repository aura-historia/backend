use common::shop_id::ShopId;
use crawler::scraper::css_selector::currency_dto::CurrencyDto;
use crawler::scraper::css_selector::product_schema::{
    ProductCssSelectorSchema, ShopsProductSchema,
};
use crawler::scraper::css_selector::product_schema_repository::{
    ShopsProductSchemaRepository, ShopsProductSchemaRepositoryImpl,
};
use crawler::scraper::css_selector::rule::{ExtractionCardinality, ExtractionKind, ExtractionRule};
use sqlx::PgPool;

use test_api::*;
use time::OffsetDateTime;
use uuid::Uuid;

const POSTGRES: Postgres = Postgres {
    migrations_dir: "src/crawler/migrations",
    setup_script: None,
};

fn minimal_css_schema() -> ProductCssSelectorSchema {
    ProductCssSelectorSchema {
        shops_product_id: Some(ExtractionRule {
            selector: "span.id".into(),
            additional_selectors: vec![],
            extract: ExtractionKind::Text,
            cardinality: ExtractionCardinality::First,
        }),
        title: ExtractionRule {
            selector: "h1".into(),
            additional_selectors: vec![],
            extract: ExtractionKind::Text,
            cardinality: ExtractionCardinality::First,
        },
        description: None,
        price: None,
        price_estimate_min: None,
        price_estimate_max: None,
        seller_name: None,
        state: ExtractionRule {
            selector: "span.state".into(),
            additional_selectors: vec![],
            extract: ExtractionKind::Text,
            cardinality: ExtractionCardinality::First,
        },
        images: ExtractionRule {
            selector: "img".into(),
            additional_selectors: vec![],
            extract: ExtractionKind::Attribute { name: "src".into() },
            cardinality: ExtractionCardinality::All,
        },
        auction_start: None,
        auction_end: None,
        default_currency: CurrencyDto::Eur,
        raw_attributes: Default::default(),
    }
}

fn full_css_schema() -> ProductCssSelectorSchema {
    ProductCssSelectorSchema {
        shops_product_id: Some(ExtractionRule {
            selector: "span#product-id".into(),
            additional_selectors: vec![],
            extract: ExtractionKind::Text,
            cardinality: ExtractionCardinality::First,
        }),
        title: ExtractionRule {
            selector: "h1.product-title".into(),
            additional_selectors: vec!["h2.alt-title".into()],
            extract: ExtractionKind::Text,
            cardinality: ExtractionCardinality::First,
        },
        description: Some(ExtractionRule {
            selector: "div.description p".into(),
            additional_selectors: vec![],
            extract: ExtractionKind::Text,
            cardinality: ExtractionCardinality::All,
        }),
        price: Some(ExtractionRule {
            selector: "span.price".into(),
            additional_selectors: vec![],
            extract: ExtractionKind::Text,
            cardinality: ExtractionCardinality::First,
        }),
        price_estimate_min: Some(ExtractionRule {
            selector: "span.estimate-min".into(),
            additional_selectors: vec![],
            extract: ExtractionKind::Text,
            cardinality: ExtractionCardinality::First,
        }),
        price_estimate_max: Some(ExtractionRule {
            selector: "span.estimate-max".into(),
            additional_selectors: vec![],
            extract: ExtractionKind::Text,
            cardinality: ExtractionCardinality::First,
        }),
        seller_name: None,
        state: ExtractionRule {
            selector: "div.availability".into(),
            additional_selectors: vec![],
            extract: ExtractionKind::Text,
            cardinality: ExtractionCardinality::First,
        },
        images: ExtractionRule {
            selector: "img.gallery-image".into(),
            additional_selectors: vec!["img.thumbnail".into()],
            extract: ExtractionKind::Attribute { name: "src".into() },
            cardinality: ExtractionCardinality::All,
        },
        auction_start: Some(ExtractionRule {
            selector: "time.auction-start".into(),
            additional_selectors: vec![],
            extract: ExtractionKind::Attribute {
                name: "datetime".into(),
            },
            cardinality: ExtractionCardinality::First,
        }),
        auction_end: Some(ExtractionRule {
            selector: "time.auction-end".into(),
            additional_selectors: vec![],
            extract: ExtractionKind::Attribute {
                name: "datetime".into(),
            },
            cardinality: ExtractionCardinality::First,
        }),
        default_currency: CurrencyDto::Eur,
        raw_attributes: Default::default(),
    }
}

fn make_shops_product_schema(
    shop_id: ShopId,
    schema: ProductCssSelectorSchema,
) -> ShopsProductSchema {
    let now = OffsetDateTime::now_utc();
    ShopsProductSchema {
        shop_id,
        product_schemas: vec![schema],
        created: now,
        updated: now,
    }
}

async fn insert_shop(pool: &PgPool, shop_id: ShopId) {
    sqlx::query("INSERT INTO shops (shop_id, created, updated) VALUES ($1, NOW(), NOW())")
        .bind(Uuid::from(shop_id))
        .execute(pool)
        .await
        .unwrap();
}

// ---------------------------------------------------------------------------
// find_product_schema
// ---------------------------------------------------------------------------

#[aura_integration_test(services = [POSTGRES])]
async fn should_return_none_when_no_schema_exists_for_find() {
    let pool = get_postgres_client().await;
    let repository = ShopsProductSchemaRepositoryImpl::new(&pool);

    let result = repository
        .find_product_schema(&ShopId::new())
        .await
        .unwrap();

    assert!(result.is_none());
}

#[aura_integration_test(services = [POSTGRES])]
async fn should_return_schema_when_exists_for_find() {
    let pool = get_postgres_client().await;
    let repository = ShopsProductSchemaRepositoryImpl::new(&pool);

    let shop_id = ShopId::new();
    insert_shop(&pool, shop_id).await;
    let schema = make_shops_product_schema(shop_id, minimal_css_schema());
    repository
        .insert_product_schema(&shop_id, &schema)
        .await
        .unwrap();

    let result = repository
        .find_product_schema(&shop_id)
        .await
        .unwrap()
        .unwrap();

    assert_eq!(result.shop_id, shop_id);
    assert_eq!(result.product_schemas[0], minimal_css_schema());
}

#[aura_integration_test(services = [POSTGRES])]
async fn should_return_none_for_unknown_shop_id_when_other_schemas_exist_for_find() {
    let pool = get_postgres_client().await;
    let repository = ShopsProductSchemaRepositoryImpl::new(&pool);

    let known_shop_id = ShopId::new();
    insert_shop(&pool, known_shop_id).await;
    let schema = make_shops_product_schema(known_shop_id, minimal_css_schema());
    repository
        .insert_product_schema(&known_shop_id, &schema)
        .await
        .unwrap();

    let unknown_shop_id = ShopId::new();
    let result = repository
        .find_product_schema(&unknown_shop_id)
        .await
        .unwrap();

    assert!(result.is_none());
}

// ---------------------------------------------------------------------------
// insert_product_schema
// ---------------------------------------------------------------------------

#[aura_integration_test(services = [POSTGRES])]
async fn should_persist_and_return_schema_when_inserting_minimal_schema_for_insert() {
    let pool = get_postgres_client().await;
    let repository = ShopsProductSchemaRepositoryImpl::new(&pool);

    let shop_id = ShopId::new();
    insert_shop(&pool, shop_id).await;
    let schema = make_shops_product_schema(shop_id, minimal_css_schema());
    let returned = repository
        .insert_product_schema(&shop_id, &schema)
        .await
        .unwrap();

    assert_eq!(returned.shop_id, shop_id);
    assert_eq!(returned.product_schemas[0], minimal_css_schema());
}

#[aura_integration_test(services = [POSTGRES])]
async fn should_persist_and_return_schema_when_inserting_full_schema_for_insert() {
    let pool = get_postgres_client().await;
    let repository = ShopsProductSchemaRepositoryImpl::new(&pool);

    let shop_id = ShopId::new();
    insert_shop(&pool, shop_id).await;
    let schema = make_shops_product_schema(shop_id, full_css_schema());
    let returned = repository
        .insert_product_schema(&shop_id, &schema)
        .await
        .unwrap();

    assert_eq!(returned.shop_id, shop_id);
    assert_eq!(returned.product_schemas[0], full_css_schema());
}

#[aura_integration_test(services = [POSTGRES])]
async fn should_preserve_created_and_updated_timestamps_for_insert() {
    let pool = get_postgres_client().await;
    let repository = ShopsProductSchemaRepositoryImpl::new(&pool);

    let shop_id = ShopId::new();
    insert_shop(&pool, shop_id).await;
    let schema = make_shops_product_schema(shop_id, minimal_css_schema());
    let created_before = schema.created;
    let updated_before = schema.updated;

    let returned = repository
        .insert_product_schema(&shop_id, &schema)
        .await
        .unwrap();

    // Timestamps must survive the round-trip (precision may be truncated to µs by Postgres)
    assert!(
        (returned.created - created_before).abs() < time::Duration::microseconds(1000),
        "created timestamp drifted unexpectedly"
    );
    assert!(
        (returned.updated - updated_before).abs() < time::Duration::microseconds(1000),
        "updated timestamp drifted unexpectedly"
    );
}

#[aura_integration_test(services = [POSTGRES])]
async fn should_allow_inserting_schemas_for_different_shop_ids_for_insert() {
    let pool = get_postgres_client().await;
    let repository = ShopsProductSchemaRepositoryImpl::new(&pool);

    let shop_id_a = ShopId::new();
    let shop_id_b = ShopId::new();
    insert_shop(&pool, shop_id_a).await;
    insert_shop(&pool, shop_id_b).await;

    repository
        .insert_product_schema(
            &shop_id_a,
            &make_shops_product_schema(shop_id_a, minimal_css_schema()),
        )
        .await
        .unwrap();
    repository
        .insert_product_schema(
            &shop_id_b,
            &make_shops_product_schema(shop_id_b, full_css_schema()),
        )
        .await
        .unwrap();

    let result_a = repository
        .find_product_schema(&shop_id_a)
        .await
        .unwrap()
        .unwrap();
    let result_b = repository
        .find_product_schema(&shop_id_b)
        .await
        .unwrap()
        .unwrap();

    assert_eq!(result_a.product_schemas[0], minimal_css_schema());
    assert_eq!(result_b.product_schemas[0], full_css_schema());
}

#[aura_integration_test(services = [POSTGRES])]
async fn should_fail_when_inserting_duplicate_shop_id_for_insert() {
    let pool = get_postgres_client().await;
    let repository = ShopsProductSchemaRepositoryImpl::new(&pool);

    let shop_id = ShopId::new();
    insert_shop(&pool, shop_id).await;
    let schema = make_shops_product_schema(shop_id, minimal_css_schema());

    repository
        .insert_product_schema(&shop_id, &schema)
        .await
        .unwrap();

    let err = repository
        .insert_product_schema(&shop_id, &schema)
        .await
        .unwrap_err();

    // Postgres unique-violation is SQLSTATE 23505, surfaced as a Database error by sqlx
    match err {
        sqlx::Error::Database(db_err) => {
            assert_eq!(
                db_err.code().as_deref(),
                Some("23505"),
                "Expected unique violation (23505), got: {:?}",
                db_err.code()
            );
        }
        other => panic!("Expected sqlx::Error::Database for PK violation, got: {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// update_product_schema
// ---------------------------------------------------------------------------

#[aura_integration_test(services = [POSTGRES])]
async fn should_replace_schema_and_refresh_updated_timestamp_for_update() {
    let pool = get_postgres_client().await;
    let repository = ShopsProductSchemaRepositoryImpl::new(&pool);

    let shop_id = ShopId::new();
    insert_shop(&pool, shop_id).await;
    let original = make_shops_product_schema(shop_id, minimal_css_schema());
    repository
        .insert_product_schema(&shop_id, &original)
        .await
        .unwrap();

    let inserted = repository
        .find_product_schema(&shop_id)
        .await
        .unwrap()
        .unwrap();

    // Small sleep so NOW() in the UPDATE is measurably later than the inserted `updated`
    tokio::time::sleep(std::time::Duration::from_millis(5)).await;

    let returned = repository
        .update_product_schema(&shop_id, &[full_css_schema()])
        .await
        .unwrap();

    assert_eq!(returned.shop_id, shop_id);
    assert_eq!(returned.product_schemas[0], full_css_schema());
    assert_ne!(returned.updated, inserted.updated);
    // created must remain unchanged
    assert!(
        (returned.created - original.created).abs() < time::Duration::microseconds(1000),
        "created timestamp must not change on update"
    );
}

#[aura_integration_test(services = [POSTGRES])]
async fn should_persist_updated_schema_so_find_returns_new_value_for_update() {
    let pool = get_postgres_client().await;
    let repository = ShopsProductSchemaRepositoryImpl::new(&pool);

    let shop_id = ShopId::new();
    insert_shop(&pool, shop_id).await;
    repository
        .insert_product_schema(
            &shop_id,
            &make_shops_product_schema(shop_id, minimal_css_schema()),
        )
        .await
        .unwrap();

    repository
        .update_product_schema(&shop_id, &[full_css_schema()])
        .await
        .unwrap();

    let found = repository
        .find_product_schema(&shop_id)
        .await
        .unwrap()
        .unwrap();

    assert_eq!(found.product_schemas[0], full_css_schema());
}

#[aura_integration_test(services = [POSTGRES])]
async fn should_return_row_not_found_when_updating_non_existent_shop_id_for_update() {
    let pool = get_postgres_client().await;
    let repository = ShopsProductSchemaRepositoryImpl::new(&pool);

    let err = repository
        .update_product_schema(&ShopId::new(), &[minimal_css_schema()])
        .await
        .unwrap_err();

    assert!(
        matches!(err, sqlx::Error::RowNotFound),
        "Expected RowNotFound when updating a shop_id that does not exist, got: {err:?}"
    );
}

#[aura_integration_test(services = [POSTGRES])]
async fn should_only_update_targeted_shop_id_and_leave_others_intact_for_update() {
    let pool = get_postgres_client().await;
    let repository = ShopsProductSchemaRepositoryImpl::new(&pool);

    let shop_id_a = ShopId::new();
    let shop_id_b = ShopId::new();
    insert_shop(&pool, shop_id_a).await;
    insert_shop(&pool, shop_id_b).await;

    repository
        .insert_product_schema(
            &shop_id_a,
            &make_shops_product_schema(shop_id_a, minimal_css_schema()),
        )
        .await
        .unwrap();
    repository
        .insert_product_schema(
            &shop_id_b,
            &make_shops_product_schema(shop_id_b, minimal_css_schema()),
        )
        .await
        .unwrap();

    repository
        .update_product_schema(&shop_id_a, &[full_css_schema()])
        .await
        .unwrap();

    let result_a = repository
        .find_product_schema(&shop_id_a)
        .await
        .unwrap()
        .unwrap();
    let result_b = repository
        .find_product_schema(&shop_id_b)
        .await
        .unwrap()
        .unwrap();

    assert_eq!(result_a.product_schemas[0], full_css_schema());
    assert_eq!(result_b.product_schemas[0], minimal_css_schema());
}

// ---------------------------------------------------------------------------
// round-trip (insert → find → update → find)
// ---------------------------------------------------------------------------

#[aura_integration_test(services = [POSTGRES])]
async fn should_preserve_all_fields_across_full_round_trip_for_repository() {
    let pool = get_postgres_client().await;
    let repository = ShopsProductSchemaRepositoryImpl::new(&pool);

    let shop_id = ShopId::new();
    insert_shop(&pool, shop_id).await;

    // 1. insert
    let inserted = repository
        .insert_product_schema(
            &shop_id,
            &make_shops_product_schema(shop_id, full_css_schema()),
        )
        .await
        .unwrap();

    assert_eq!(inserted.shop_id, shop_id);
    assert_eq!(inserted.product_schemas[0], full_css_schema());

    // 2. find after insert
    let found_after_insert = repository
        .find_product_schema(&shop_id)
        .await
        .unwrap()
        .unwrap();

    assert_eq!(found_after_insert.shop_id, shop_id);
    assert_eq!(found_after_insert.product_schemas[0], full_css_schema());

    tokio::time::sleep(std::time::Duration::from_millis(500)).await;

    // 3. update
    let updated = repository
        .update_product_schema(&shop_id, &[minimal_css_schema()])
        .await
        .unwrap();

    assert_eq!(updated.product_schemas[0], minimal_css_schema());
    assert_ne!(updated.updated, inserted.updated);

    // 4. find after update
    let found_after_update = repository
        .find_product_schema(&shop_id)
        .await
        .unwrap()
        .unwrap();

    assert_eq!(found_after_update.product_schemas[0], minimal_css_schema());
    assert_eq!(found_after_update.created, found_after_insert.created);
}

#[aura_integration_test(services = [POSTGRES])]
async fn should_delete_product_schema_when_parent_shop_is_deleted() {
    let pool = get_postgres_client().await;
    let repository = ShopsProductSchemaRepositoryImpl::new(&pool);

    let shop_id = ShopId::new();
    insert_shop(&pool, shop_id).await;

    repository
        .insert_product_schema(
            &shop_id,
            &make_shops_product_schema(shop_id, minimal_css_schema()),
        )
        .await
        .unwrap();

    sqlx::query("DELETE FROM shops WHERE shop_id = $1")
        .bind(Uuid::from(shop_id))
        .execute(&pool)
        .await
        .unwrap();

    let found = repository.find_product_schema(&shop_id).await.unwrap();
    assert!(found.is_none());
}
