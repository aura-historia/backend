use domain_primitives::event_id::EventId;
use platform_postgres::SqlxTransaction;
use product_listing_service::ports::{
    ProductListingTranslationWrite, ProductListingTranslationWriteError,
    ProductListingTranslationWriteOutcome, ProductListingTranslationWriter,
    ProductListingTranslationWriterFactory,
};
use serde_json::json;
use sqlx::PgConnection;

#[derive(Debug, Clone, Copy, Default)]
pub struct SqlxProductListingTranslationWriterFactory;

struct SqlxProductListingTranslationWriter<'tx> {
    connection: &'tx mut PgConnection,
}

#[derive(Debug, thiserror::Error)]
#[error("product translation SQL write failed")]
struct ProductListingTranslationWriteSqlxError(#[source] sqlx::Error);

impl SqlxProductListingTranslationWriterFactory {
    pub fn new() -> Self {
        Self
    }
}

impl ProductListingTranslationWriterFactory<SqlxTransaction>
    for SqlxProductListingTranslationWriterFactory
{
    fn in_transaction<'tx>(
        &'tx self,
        tx: &'tx mut SqlxTransaction,
    ) -> impl ProductListingTranslationWriter + 'tx {
        SqlxProductListingTranslationWriter {
            connection: tx.connection(),
        }
    }
}

#[async_trait::async_trait]
impl ProductListingTranslationWriter for SqlxProductListingTranslationWriter<'_> {
    async fn apply(
        &mut self,
        write: &ProductListingTranslationWrite,
    ) -> Result<ProductListingTranslationWriteOutcome, ProductListingTranslationWriteError> {
        let content_source_event_id = sqlx::query_scalar::<_, uuid::Uuid>(
            "SELECT content_source_event_id FROM product_listings WHERE product_listing_id = $1 FOR UPDATE",
        )
        .bind(uuid::Uuid::from(write.product_listing_id))
        .fetch_optional(&mut *self.connection)
        .await
        .map_err(ProductListingTranslationWriteSqlxError)?;

        let Some(content_source_event_id) = content_source_event_id else {
            return Ok(ProductListingTranslationWriteOutcome::ProductListingNotFound);
        };
        if duplicate_translation_exists(self.connection, write).await? {
            return Ok(ProductListingTranslationWriteOutcome::Duplicate);
        }
        if EventId::from(content_source_event_id) != write.source_event_id {
            return Ok(ProductListingTranslationWriteOutcome::Stale);
        }

        for (language, title) in &write.titles {
            sqlx::query(
                r#"
                INSERT INTO product_listing_translations (product_listing_id, language, title, source_event_id)
                VALUES ($1, $2, $3, $4)
                ON CONFLICT (product_listing_id, language)
                DO UPDATE SET
                    title = EXCLUDED.title,
                    source_event_id = EXCLUDED.source_event_id,
                    updated = now()
                "#,
            )
            .bind(uuid::Uuid::from(write.product_listing_id))
            .bind(language.as_str())
            .bind(title.as_ref())
            .bind(uuid::Uuid::from(write.source_event_id))
            .execute(&mut *self.connection)
            .await
            .map_err(ProductListingTranslationWriteSqlxError)?;
        }

        let target_languages = write
            .titles
            .keys()
            .map(|language| language.as_str())
            .collect::<Vec<_>>();
        let payload = json!({
            "sourceEventId": write.source_event_id.to_string(),
            "sourceLanguage": write.source_language.as_str(),
            "targetLanguages": target_languages,
        });
        sqlx::query(
            r#"
            INSERT INTO product_listing_events (
                event_id, product_listing_id, event_type, event_group, event_type_schema_version,
                payload, event_time
            ) VALUES ($1, $2, 'ENRICHMENT_TRANSLATED_TITLES', 'ENRICHMENT', 1, $3, now())
            "#,
        )
        .bind(uuid::Uuid::from(write.enrichment_event_id))
        .bind(uuid::Uuid::from(write.product_listing_id))
        .bind(payload)
        .execute(&mut *self.connection)
        .await
        .map_err(ProductListingTranslationWriteSqlxError)?;

        let update = sqlx::query(
            "UPDATE product_listings SET current_event_id = $1, projection_version = projection_version + 1, updated = now() WHERE product_listing_id = $2 AND content_source_event_id = $3",
        )
        .bind(uuid::Uuid::from(write.enrichment_event_id))
        .bind(uuid::Uuid::from(write.product_listing_id))
        .bind(uuid::Uuid::from(write.source_event_id))
        .execute(&mut *self.connection)
        .await
        .map_err(ProductListingTranslationWriteSqlxError)?;
        if update.rows_affected() != 1 {
            return Err(ProductListingTranslationWriteError::WriteFailed {
                source: application::error::static_error(
                    "locked product translation source revision changed unexpectedly",
                ),
            });
        }

        Ok(ProductListingTranslationWriteOutcome::Applied)
    }
}

async fn duplicate_translation_exists(
    connection: &mut PgConnection,
    write: &ProductListingTranslationWrite,
) -> Result<bool, ProductListingTranslationWriteError> {
    if write.titles.is_empty() {
        return Ok(false);
    }
    let expected_count = i64::try_from(write.titles.len()).map_err(|_| {
        ProductListingTranslationWriteError::WriteFailed {
            source: application::error::static_error(
                "product translation count exceeds PostgreSQL range",
            ),
        }
    })?;
    let matched_count = sqlx::query_scalar::<_, i64>(
        r#"
        SELECT count(*)
        FROM product_listing_translations
        WHERE product_listing_id = $1
          AND source_event_id = $2
          AND (language, title) IN (
              SELECT * FROM unnest($3::text[], $4::text[])
          )
        "#,
    )
    .bind(uuid::Uuid::from(write.product_listing_id))
    .bind(uuid::Uuid::from(write.source_event_id))
    .bind(
        write
            .titles
            .keys()
            .map(|language| language.as_str())
            .collect::<Vec<_>>(),
    )
    .bind(write.titles.values().map(AsRef::as_ref).collect::<Vec<_>>())
    .fetch_one(&mut *connection)
    .await
    .map_err(ProductListingTranslationWriteSqlxError)?;
    Ok(matched_count == expected_count)
}

impl From<ProductListingTranslationWriteSqlxError> for ProductListingTranslationWriteError {
    fn from(source: ProductListingTranslationWriteSqlxError) -> Self {
        Self::WriteFailed {
            source: application::error::box_error(source),
        }
    }
}
