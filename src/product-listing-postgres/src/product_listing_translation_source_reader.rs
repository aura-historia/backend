use application::error::{box_error, static_error};
use domain_primitives::event_id::EventId;
use localization::Language;
use product_listing_core::{product_listing_id::ProductListingId, title::Title};
use product_listing_service::ports::{
    ProductListingTranslationSource, ProductListingTranslationSourceReadError,
    ProductListingTranslationSourceReader,
};
use sqlx::PgPool;

#[derive(Clone)]
pub struct SqlxProductListingTranslationSourceReader {
    pool: PgPool,
}

#[derive(Debug, sqlx::FromRow)]
struct ProductListingTranslationSourceRow {
    product_id: uuid::Uuid,
    event_id: uuid::Uuid,
    current_event_id: uuid::Uuid,
    event_type: String,
    title_text: Option<String>,
    title_language: Option<String>,
}

#[derive(Debug, thiserror::Error)]
#[error("product translation source SQL query failed")]
struct ProductListingTranslationSourceQueryError(#[source] sqlx::Error);

#[derive(Debug, thiserror::Error)]
#[error("product translation source row is invalid")]
struct ProductListingTranslationSourceMappingError {
    #[source]
    source: application::error::BoxError,
}

impl From<ProductListingTranslationSourceQueryError> for ProductListingTranslationSourceReadError {
    fn from(source: ProductListingTranslationSourceQueryError) -> Self {
        Self::QueryFailed {
            source: box_error(source),
        }
    }
}

impl SqlxProductListingTranslationSourceReader {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait::async_trait]
impl ProductListingTranslationSourceReader for SqlxProductListingTranslationSourceReader {
    async fn find_source(
        &self,
        event_id: EventId,
        product_id: ProductListingId,
    ) -> Result<Option<ProductListingTranslationSource>, ProductListingTranslationSourceReadError>
    {
        let row = sqlx::query_as::<_, ProductListingTranslationSourceRow>(
            r#"
            SELECT
                event.event_id,
                event.product_id,
                event.event_type,
                product.event_id AS current_event_id,
                product.title_text,
                product.title_language
            FROM product_events event
            JOIN products product ON product.product_id = event.product_id
            WHERE event.event_id = $1
              AND event.product_id = $2
            "#,
        )
        .bind(uuid::Uuid::from(event_id))
        .bind(uuid::Uuid::from(product_id))
        .fetch_optional(&self.pool)
        .await
        .map_err(ProductListingTranslationSourceQueryError)?;

        row.map(TryInto::try_into).transpose()
    }
}

impl TryFrom<ProductListingTranslationSourceRow> for ProductListingTranslationSource {
    type Error = ProductListingTranslationSourceReadError;

    fn try_from(row: ProductListingTranslationSourceRow) -> Result<Self, Self::Error> {
        let (title, title_language) = match (row.title_text, row.title_language) {
            (Some(raw_title), Some(raw_language)) => {
                let title = Title::from(raw_title.as_str());
                if title.as_ref().is_empty() || title.as_ref() != raw_title {
                    return Err(mapping_error(
                        "persisted product translation title is invalid",
                    ));
                }
                (Some(title), Some(parse_language(&raw_language)?))
            }
            (None, None) => (None, None),
            _ => {
                return Err(mapping_error(
                    "persisted product translation title is incomplete",
                ));
            }
        };

        Ok(Self {
            product_id: ProductListingId::from(row.product_id),
            event_id: EventId::from(row.event_id),
            current_event_id: EventId::from(row.current_event_id),
            event_type: row.event_type,
            title,
            title_language,
        })
    }
}

fn parse_language(value: &str) -> Result<Language, ProductListingTranslationSourceReadError> {
    Language::from_code(value)
        .ok_or_else(|| mapping_error("persisted product translation language is invalid"))
}

fn mapping_error(message: &'static str) -> ProductListingTranslationSourceReadError {
    ProductListingTranslationSourceReadError::InvalidPersistedState {
        source: box_error(ProductListingTranslationSourceMappingError {
            source: static_error(message),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_reject_noncanonical_translation_source_language() {
        assert!(parse_language("EN").is_err());
    }
}
