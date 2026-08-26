use application::error::{box_error, static_error};
use domain_primitives::event_id::EventId;
use platform_postgres::SqlxTransaction;
use product_listing_core::content_policy::{ContentPolicyDecision, SensitiveContentCategory};
use product_listing_service::ports::{
    ProductListingContentAssessmentWrite, ProductListingContentAssessmentWriteError,
    ProductListingContentAssessmentWriteOutcome, ProductListingContentAssessmentWriter,
    ProductListingContentAssessmentWriterFactory,
};
use sqlx::PgConnection;

#[derive(Debug, Clone, Copy, Default)]
pub struct SqlxProductListingContentAssessmentWriterFactory;

struct SqlxProductListingContentAssessmentWriter<'tx> {
    connection: &'tx mut PgConnection,
}

#[derive(Debug, sqlx::FromRow)]
struct StoredContentAssessmentRow {
    source_event_id: uuid::Uuid,
    decision: String,
    category: Option<String>,
}

#[derive(Debug, thiserror::Error)]
#[error("product content assessment SQL write failed")]
struct ProductListingContentAssessmentWriteSqlxError(#[source] sqlx::Error);

#[derive(Debug, thiserror::Error)]
#[error("stored product content assessment is invalid")]
struct ProductListingContentAssessmentWriteMappingError {
    #[source]
    source: application::error::BoxError,
}

impl SqlxProductListingContentAssessmentWriterFactory {
    pub fn new() -> Self {
        Self
    }
}

impl ProductListingContentAssessmentWriterFactory<SqlxTransaction>
    for SqlxProductListingContentAssessmentWriterFactory
{
    fn in_transaction<'tx>(
        &'tx self,
        tx: &'tx mut SqlxTransaction,
    ) -> impl ProductListingContentAssessmentWriter + 'tx {
        SqlxProductListingContentAssessmentWriter {
            connection: tx.connection(),
        }
    }
}

#[async_trait::async_trait]
impl ProductListingContentAssessmentWriter for SqlxProductListingContentAssessmentWriter<'_> {
    async fn apply(
        &mut self,
        write: &ProductListingContentAssessmentWrite,
    ) -> Result<
        ProductListingContentAssessmentWriteOutcome,
        ProductListingContentAssessmentWriteError,
    > {
        let current_event_id = sqlx::query_scalar::<_, uuid::Uuid>(
            "SELECT event_id FROM product_listings WHERE product_listing_id = $1 FOR UPDATE",
        )
        .bind(uuid::Uuid::from(write.product_listing_id))
        .fetch_optional(&mut *self.connection)
        .await
        .map_err(ProductListingContentAssessmentWriteSqlxError)?;
        let Some(current_event_id) = current_event_id else {
            return Ok(ProductListingContentAssessmentWriteOutcome::ProductListingNotFound);
        };

        if EventId::from(current_event_id) != write.source_event_id {
            return Ok(
                if stored_assessment_matches(self.connection, write).await? {
                    ProductListingContentAssessmentWriteOutcome::Duplicate
                } else {
                    ProductListingContentAssessmentWriteOutcome::Stale
                },
            );
        }

        let stored_assessment_matches = stored_assessment_matches(self.connection, write).await?;

        let Some(decision) = write.decision else {
            sqlx::query(
                "DELETE FROM product_listing_content_assessments WHERE product_listing_id = $1",
            )
            .bind(uuid::Uuid::from(write.product_listing_id))
            .execute(&mut *self.connection)
            .await
            .map_err(ProductListingContentAssessmentWriteSqlxError)?;
            return Ok(ProductListingContentAssessmentWriteOutcome::Cleared);
        };

        if stored_assessment_matches {
            return Ok(ProductListingContentAssessmentWriteOutcome::Duplicate);
        }

        let (decision, category) = encode_decision(decision);
        sqlx::query(
            r#"
            INSERT INTO product_listing_content_assessments (
                product_listing_id, source_event_id, decision, category
            ) VALUES ($1, $2, $3, $4)
            ON CONFLICT (product_listing_id) DO UPDATE SET
                source_event_id = EXCLUDED.source_event_id,
                decision = EXCLUDED.decision,
                category = EXCLUDED.category,
                updated = now()
            "#,
        )
        .bind(uuid::Uuid::from(write.product_listing_id))
        .bind(uuid::Uuid::from(write.source_event_id))
        .bind(decision)
        .bind(category)
        .execute(&mut *self.connection)
        .await
        .map_err(ProductListingContentAssessmentWriteSqlxError)?;

        Ok(ProductListingContentAssessmentWriteOutcome::Applied)
    }
}

async fn stored_assessment_matches(
    connection: &mut PgConnection,
    write: &ProductListingContentAssessmentWrite,
) -> Result<bool, ProductListingContentAssessmentWriteError> {
    let stored = sqlx::query_as::<_, StoredContentAssessmentRow>(
        r#"
        SELECT source_event_id, decision, category
        FROM product_listing_content_assessments
        WHERE product_listing_id = $1
        "#,
    )
    .bind(uuid::Uuid::from(write.product_listing_id))
    .fetch_optional(&mut *connection)
    .await
    .map_err(ProductListingContentAssessmentWriteSqlxError)?;

    let Some(stored) = stored else {
        return Ok(false);
    };
    let stored_decision = decode_stored_decision(&stored.decision, stored.category.as_deref())?;
    Ok(
        stored.source_event_id == uuid::Uuid::from(write.source_event_id)
            && Some(stored_decision) == write.decision,
    )
}

fn encode_decision(decision: ContentPolicyDecision) -> (&'static str, Option<&'static str>) {
    match decision {
        ContentPolicyDecision::Allowed => (decision.as_str(), None),
        ContentPolicyDecision::RequiresConsent(category) => {
            (decision.as_str(), Some(category.as_str()))
        }
    }
}

fn decode_stored_decision(
    decision: &str,
    category: Option<&str>,
) -> Result<ContentPolicyDecision, ProductListingContentAssessmentWriteError> {
    match (decision, category) {
        ("ALLOWED", None) => Ok(ContentPolicyDecision::Allowed),
        ("REQUIRES_CONSENT", Some(category)) => SensitiveContentCategory::from_code(category)
            .map(ContentPolicyDecision::RequiresConsent)
            .ok_or_else(|| mapping_error("stored product content assessment category is invalid")),
        _ => Err(mapping_error(
            "stored product content assessment decision and category are invalid",
        )),
    }
}

fn mapping_error(message: &'static str) -> ProductListingContentAssessmentWriteError {
    ProductListingContentAssessmentWriteError::WriteFailed {
        source: box_error(ProductListingContentAssessmentWriteMappingError {
            source: static_error(message),
        }),
    }
}

impl From<ProductListingContentAssessmentWriteSqlxError>
    for ProductListingContentAssessmentWriteError
{
    fn from(source: ProductListingContentAssessmentWriteSqlxError) -> Self {
        Self::WriteFailed {
            source: box_error(source),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_encode_exact_content_assessment_codecs() {
        assert_eq!(
            ("ALLOWED", None),
            encode_decision(ContentPolicyDecision::Allowed)
        );
        assert_eq!(
            ("REQUIRES_CONSENT", Some("NAZI_GERMANY")),
            encode_decision(ContentPolicyDecision::RequiresConsent(
                SensitiveContentCategory::NaziGermany
            ))
        );
    }

    #[test]
    fn should_reject_invalid_stored_content_assessment_codecs() {
        assert!(decode_stored_decision("ALLOWED", Some("NAZI_GERMANY")).is_err());
        assert!(decode_stored_decision("REQUIRES_CONSENT", None).is_err());
        assert!(decode_stored_decision("allowed", None).is_err());
    }
}
