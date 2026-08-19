use common::event_id::EventId;
use platform_postgres::SqlxTransaction;
use product_service::ports::{
    ProductTranslationWrite, ProductTranslationWriteError, ProductTranslationWriteOutcome,
    ProductTranslationWriter, ProductTranslationWriterFactory,
};
use serde_json::json;
use sqlx::PgConnection;

#[derive(Debug, Clone, Copy, Default)]
pub struct SqlxProductTranslationWriterFactory;

struct SqlxProductTranslationWriter<'tx> {
    connection: &'tx mut PgConnection,
}

#[derive(Debug, thiserror::Error)]
#[error("product translation SQL write failed")]
struct ProductTranslationWriteSqlxError(#[source] sqlx::Error);

impl SqlxProductTranslationWriterFactory {
    pub fn new() -> Self {
        Self
    }
}

impl ProductTranslationWriterFactory<SqlxTransaction> for SqlxProductTranslationWriterFactory {
    fn in_transaction<'tx>(
        &'tx self,
        tx: &'tx mut SqlxTransaction,
    ) -> impl ProductTranslationWriter + 'tx {
        SqlxProductTranslationWriter {
            connection: tx.connection(),
        }
    }
}

#[async_trait::async_trait]
impl ProductTranslationWriter for SqlxProductTranslationWriter<'_> {
    async fn apply(
        &mut self,
        write: &ProductTranslationWrite,
    ) -> Result<ProductTranslationWriteOutcome, ProductTranslationWriteError> {
        let current_event_id = sqlx::query_scalar::<_, uuid::Uuid>(
            "SELECT event_id FROM products WHERE product_id = $1 FOR UPDATE",
        )
        .bind(uuid::Uuid::from(write.product_id))
        .fetch_optional(&mut *self.connection)
        .await
        .map_err(ProductTranslationWriteSqlxError)?;

        let Some(current_event_id) = current_event_id else {
            return Ok(ProductTranslationWriteOutcome::ProductNotFound);
        };
        if EventId::from(current_event_id) != write.source_event_id {
            return Ok(
                if duplicate_translation_exists(self.connection, write).await? {
                    ProductTranslationWriteOutcome::Duplicate
                } else {
                    ProductTranslationWriteOutcome::Stale
                },
            );
        }

        for (language, title) in &write.titles {
            sqlx::query(
                r#"
                INSERT INTO product_translations (product_id, language, title, source_event_id)
                VALUES ($1, $2, $3, $4)
                ON CONFLICT (product_id, language)
                DO UPDATE SET
                    title = EXCLUDED.title,
                    source_event_id = EXCLUDED.source_event_id,
                    updated = now()
                "#,
            )
            .bind(uuid::Uuid::from(write.product_id))
            .bind(language.as_str())
            .bind(title.as_ref())
            .bind(uuid::Uuid::from(write.source_event_id))
            .execute(&mut *self.connection)
            .await
            .map_err(ProductTranslationWriteSqlxError)?;
        }

        let titles = write
            .titles
            .iter()
            .map(|(language, title)| {
                (
                    language.as_str().to_owned(),
                    serde_json::Value::String(title.as_ref().to_owned()),
                )
            })
            .collect::<serde_json::Map<String, serde_json::Value>>();
        let payload = json!({
            "kind": "translatedTitles",
            "sourceEventId": write.source_event_id.to_string(),
            "sourceLanguage": write.source_language.as_str(),
            "titles": titles,
        });
        sqlx::query(
            r#"
            INSERT INTO product_events (
                event_id, product_id, event_type, event_group, payload, event_time
            ) VALUES ($1, $2, 'ENRICHMENT_TRANSLATED_TITLES', 'ENRICHMENT', $3, now())
            "#,
        )
        .bind(uuid::Uuid::from(write.enrichment_event_id))
        .bind(uuid::Uuid::from(write.product_id))
        .bind(payload)
        .execute(&mut *self.connection)
        .await
        .map_err(ProductTranslationWriteSqlxError)?;

        let update = sqlx::query(
            "UPDATE products SET event_id = $1, projection_version = projection_version + 1, updated = now() WHERE product_id = $2 AND event_id = $3",
        )
        .bind(uuid::Uuid::from(write.enrichment_event_id))
        .bind(uuid::Uuid::from(write.product_id))
        .bind(uuid::Uuid::from(write.source_event_id))
        .execute(&mut *self.connection)
        .await
        .map_err(ProductTranslationWriteSqlxError)?;
        if update.rows_affected() != 1 {
            return Err(ProductTranslationWriteError::WriteFailed {
                source: common::error::boxed::static_error(
                    "locked product translation source revision changed unexpectedly",
                ),
            });
        }

        Ok(ProductTranslationWriteOutcome::Applied)
    }
}

async fn duplicate_translation_exists(
    connection: &mut PgConnection,
    write: &ProductTranslationWrite,
) -> Result<bool, ProductTranslationWriteError> {
    if write.titles.is_empty() {
        return Ok(false);
    }
    let expected_count = i64::try_from(write.titles.len()).map_err(|_| {
        ProductTranslationWriteError::WriteFailed {
            source: common::error::boxed::static_error(
                "product translation count exceeds PostgreSQL range",
            ),
        }
    })?;
    let matched_count = sqlx::query_scalar::<_, i64>(
        r#"
        SELECT count(*)
        FROM product_translations
        WHERE product_id = $1
          AND source_event_id = $2
          AND (language, title) IN (
              SELECT * FROM unnest($3::text[], $4::text[])
          )
        "#,
    )
    .bind(uuid::Uuid::from(write.product_id))
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
    .map_err(ProductTranslationWriteSqlxError)?;
    Ok(matched_count == expected_count)
}

impl From<ProductTranslationWriteSqlxError> for ProductTranslationWriteError {
    fn from(source: ProductTranslationWriteSqlxError) -> Self {
        Self::WriteFailed {
            source: common::error::boxed::box_error(source),
        }
    }
}
