//! What a listing's title translations do when its content revision advances
//! (docs/product-listing.md#title-translation). Runs inside the listing update's transaction.

use domain_primitives::event_id::EventId;
use product_listing_core::product_listing_id::ProductListingId;
use sqlx::PgConnection;

/// Drops every translation of the listing; the new revision's translation job replaces them.
pub(crate) async fn discard_title_translations(
    connection: &mut PgConnection,
    product_listing_id: ProductListingId,
) -> Result<(), sqlx::Error> {
    sqlx::query("DELETE FROM product_listing_translations WHERE product_listing_id = $1")
        .bind(product_listing_id.as_uuid())
        .execute(connection)
        .await?;
    Ok(())
}

/// Moves the translations of `replaced` onto `revision`, whose title is the same, without a model
/// call. Only a revision with a translated-titles completion is carried: the completion is copied
/// for `revision` and becomes the listing's current event, so duplicate detection and the search
/// projection treat it like a translation that just landed. Rows of any other revision are stale
/// and dropped. Translation work still pending or parked for `replaced` moves along as well,
/// keeping its batch assignment, attempts and parking.
pub(crate) async fn carry_title_translations(
    connection: &mut PgConnection,
    product_listing_id: ProductListingId,
    replaced: EventId,
    revision: EventId,
) -> Result<(), sqlx::Error> {
    let completion = sqlx::query_scalar::<_, uuid::Uuid>(
        r#"
        INSERT INTO product_listing_events (
            event_id, product_listing_id, event_type, event_group, event_type_schema_version,
            payload, event_time
        )
        SELECT $1, product_listing_id, event_type, event_group, event_type_schema_version,
               jsonb_set(payload, '{sourceEventId}', to_jsonb($4::text)), now()
        FROM product_listing_events
        WHERE product_listing_id = $2
          AND event_type = 'ENRICHMENT_TRANSLATED_TITLES'
          AND event_group = 'ENRICHMENT'
          AND event_type_schema_version = 1
          AND payload ->> 'sourceEventId' = $3
        ORDER BY event_id DESC
        LIMIT 1
        RETURNING event_id
        "#,
    )
    .bind(EventId::new().as_uuid())
    .bind(product_listing_id.as_uuid())
    .bind(replaced.as_uuid().to_string())
    .bind(revision.as_uuid().to_string())
    .fetch_optional(&mut *connection)
    .await?;

    match completion {
        Some(completion) => {
            sqlx::query(
                r#"
                UPDATE product_listing_translations
                SET source_event_id = $3,
                    updated = aura_advance_updated(updated)
                WHERE product_listing_id = $1 AND source_event_id = $2
                "#,
            )
            .bind(product_listing_id.as_uuid())
            .bind(replaced.as_uuid())
            .bind(revision.as_uuid())
            .execute(&mut *connection)
            .await?;
            sqlx::query(
                "DELETE FROM product_listing_translations WHERE product_listing_id = $1 AND source_event_id <> $2",
            )
            .bind(product_listing_id.as_uuid())
            .bind(revision.as_uuid())
            .execute(&mut *connection)
            .await?;
            sqlx::query(
                r#"
                UPDATE product_listings
                SET current_event_id = $2,
                    projection_version = projection_version + 1,
                    updated = aura_advance_updated(updated)
                WHERE product_listing_id = $1
                "#,
            )
            .bind(product_listing_id.as_uuid())
            .bind(completion)
            .execute(&mut *connection)
            .await?;
        }
        None => discard_title_translations(connection, product_listing_id).await?,
    }

    sqlx::query(
        r#"
        UPDATE product_listing_title_translation_work
        SET source_event_id = $3
        WHERE product_listing_id = $1 AND source_event_id = $2
        "#,
    )
    .bind(product_listing_id.as_uuid())
    .bind(replaced.as_uuid())
    .bind(revision.as_uuid())
    .execute(connection)
    .await?;
    Ok(())
}
