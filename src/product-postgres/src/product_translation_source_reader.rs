use common::{
    error::boxed::{box_error, static_error},
    event_id::EventId,
    product_id::ProductId,
};
use localization::Language;
use product_core::title::Title;
use product_service::ports::{
    ProductTranslationSource, ProductTranslationSourceReadError, ProductTranslationSourceReader,
};
use sqlx::PgPool;

#[derive(Clone)]
pub struct SqlxProductTranslationSourceReader {
    pool: PgPool,
}

#[derive(Debug, sqlx::FromRow)]
struct ProductTranslationSourceRow {
    product_id: uuid::Uuid,
    event_id: uuid::Uuid,
    current_event_id: uuid::Uuid,
    event_type: String,
    title_text: Option<String>,
    title_language: Option<String>,
}

#[derive(Debug, thiserror::Error)]
#[error("product translation source SQL query failed")]
struct ProductTranslationSourceQueryError(#[source] sqlx::Error);

#[derive(Debug, thiserror::Error)]
#[error("product translation source row is invalid")]
struct ProductTranslationSourceMappingError {
    #[source]
    source: common::error::boxed::BoxError,
}

impl From<ProductTranslationSourceQueryError> for ProductTranslationSourceReadError {
    fn from(source: ProductTranslationSourceQueryError) -> Self {
        Self::QueryFailed {
            source: box_error(source),
        }
    }
}

impl SqlxProductTranslationSourceReader {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait::async_trait]
impl ProductTranslationSourceReader for SqlxProductTranslationSourceReader {
    async fn find_source(
        &self,
        event_id: EventId,
        product_id: ProductId,
    ) -> Result<Option<ProductTranslationSource>, ProductTranslationSourceReadError> {
        let row = sqlx::query_as::<_, ProductTranslationSourceRow>(
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
        .map_err(ProductTranslationSourceQueryError)?;

        row.map(TryInto::try_into).transpose()
    }
}

impl TryFrom<ProductTranslationSourceRow> for ProductTranslationSource {
    type Error = ProductTranslationSourceReadError;

    fn try_from(row: ProductTranslationSourceRow) -> Result<Self, Self::Error> {
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
            product_id: ProductId::from(row.product_id),
            event_id: EventId::from(row.event_id),
            current_event_id: EventId::from(row.current_event_id),
            event_type: row.event_type,
            title,
            title_language,
        })
    }
}

fn parse_language(value: &str) -> Result<Language, ProductTranslationSourceReadError> {
    match value {
        "de" => Ok(Language::De),
        "en" => Ok(Language::En),
        "fr" => Ok(Language::Fr),
        "es" => Ok(Language::Es),
        "it" => Ok(Language::It),
        "zh" => Ok(Language::Zh),
        "pt" => Ok(Language::Pt),
        "pl" => Ok(Language::Pl),
        "tr" => Ok(Language::Tr),
        "nl" => Ok(Language::Nl),
        "cs" => Ok(Language::Cs),
        "ja" => Ok(Language::Ja),
        "ru" => Ok(Language::Ru),
        "ar" => Ok(Language::Ar),
        _ => Err(mapping_error(
            "persisted product translation language is invalid",
        )),
    }
}

fn mapping_error(message: &'static str) -> ProductTranslationSourceReadError {
    ProductTranslationSourceReadError::InvalidPersistedState {
        source: box_error(ProductTranslationSourceMappingError {
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
