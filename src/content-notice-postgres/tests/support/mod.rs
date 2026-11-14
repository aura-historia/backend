//! Seeds the business rows a content notice can target.

use listing_source_core::ListingSourceId;
use product_listing_core::{
    listing_lifecycle::ListingLifecycle, product_listing_id::ProductListingId,
};
use sqlx::PgPool;

/// Seeds a listing source whose operator holds a partnership in `partnership_state`, if any.
pub async fn seed_listing_source(
    pool: &PgPool,
    name: &str,
    url: Option<&str>,
    partnership_state: Option<&str>,
) -> ListingSourceId {
    let party_id = uuid::Uuid::now_v7();
    let listing_source_id = ListingSourceId::new();
    sqlx::query("INSERT INTO parties (party_id, party_slug_id, name) VALUES ($1, $2, $3)")
        .bind(party_id)
        .bind(format!("party-{}", party_id.simple()))
        .bind(format!("{name} operator"))
        .execute(pool)
        .await
        .unwrap_or_else(|error| panic!("insert party: {error}"));
    if let Some(state) = partnership_state {
        sqlx::query(
            "INSERT INTO partnerships (partnership_id, party_id, business_state) VALUES ($1, $2, $3)",
        )
        .bind(uuid::Uuid::now_v7())
        .bind(party_id)
        .bind(state)
        .execute(pool)
        .await
        .unwrap_or_else(|error| panic!("insert partnership: {error}"));
    }
    sqlx::query("INSERT INTO listing_sources (listing_source_id, listing_source_slug_id, name, operator_party_id, url, listing_source_type) VALUES ($1, $2, $3, $4, $5, 'DEALER')")
        .bind(listing_source_id.into_uuid())
        .bind(format!("source-{}", listing_source_id.as_uuid().simple()))
        .bind(name)
        .bind(party_id)
        .bind(url)
        .execute(pool)
        .await
        .unwrap_or_else(|error| panic!("insert listing source: {error}"));
    listing_source_id
}

pub async fn seed_product_listing(
    pool: &PgPool,
    listing_source_id: ListingSourceId,
    lifecycle: ListingLifecycle,
    title: Option<&str>,
) -> ProductListingId {
    let product_listing_id = ProductListingId::new();
    let event_id = uuid::Uuid::now_v7();
    let mut tx = pool.begin().await.unwrap();
    sqlx::query("INSERT INTO product_listings (product_listing_id, product_listing_title_slug_id, current_event_id, content_source_event_id, embedding_source_event_id, listing_source_id, source_listing_id, title_text, title_language, lifecycle, url) VALUES ($1, $2, $3, $3, $3, $4, $5, $6, $7, $8, 'https://shop.example/vase?id=42')")
        .bind(product_listing_id.into_uuid())
        .bind(format!("vase-{}", &product_listing_id.as_uuid().simple().to_string()[26..]))
        .bind(event_id)
        .bind(listing_source_id.into_uuid())
        .bind(product_listing_id.to_string())
        .bind(title)
        .bind(title.map(|_| "de"))
        .bind(lifecycle.as_str())
        .execute(&mut *tx)
        .await
        .unwrap_or_else(|error| panic!("insert product listing: {error}"));
    sqlx::query("INSERT INTO product_listing_events (event_id, product_listing_id, event_type, event_group, event_type_schema_version, payload, event_time) VALUES ($1, $2, 'PRODUCT_LISTING_DISCOVERED', 'DOMAIN', 1, '{}', now())")
        .bind(event_id)
        .bind(product_listing_id.into_uuid())
        .execute(&mut *tx)
        .await
        .unwrap_or_else(|error| panic!("insert product listing event: {error}"));
    tx.commit().await.unwrap();
    product_listing_id
}
