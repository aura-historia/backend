use application::error::{box_error, static_error};
use domain_primitives::event_id::EventId;
use localization::Language;
use localization::Localized;
use product_core::{description::Description, product_id::ProductId, title::Title};
use product_service::ports::{
    ProductEmbeddingSource, ProductEmbeddingSourceReadError, ProductEmbeddingSourceReader,
};
use serde::Deserialize;
use sqlx::PgPool;
use url::Url;

#[derive(Clone)]
pub struct SqlxProductEmbeddingSourceReader {
    pool: PgPool,
}

#[derive(Debug, sqlx::FromRow)]
struct ProductEmbeddingSourceRow {
    product_id: uuid::Uuid,
    event_id: uuid::Uuid,
    current_event_id: uuid::Uuid,
    event_type: String,
    title_text: Option<String>,
    title_language: Option<String>,
    description_text: Option<String>,
    description_language: Option<String>,
    product_images: serde_json::Value,
}

#[derive(Debug, Deserialize)]
struct ProductImageRecord {
    url: String,
}

#[derive(Debug, thiserror::Error)]
#[error("product embedding source SQL query failed")]
struct ProductEmbeddingSourceQueryError(#[source] sqlx::Error);

#[derive(Debug, thiserror::Error)]
#[error("product embedding source row is invalid")]
struct ProductEmbeddingSourceMappingError {
    #[source]
    source: application::error::BoxError,
}

impl SqlxProductEmbeddingSourceReader {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait::async_trait]
impl ProductEmbeddingSourceReader for SqlxProductEmbeddingSourceReader {
    async fn find_source(
        &self,
        event_id: EventId,
        product_id: ProductId,
    ) -> Result<Option<ProductEmbeddingSource>, ProductEmbeddingSourceReadError> {
        let row = sqlx::query_as::<_, ProductEmbeddingSourceRow>(
            r#"
            SELECT event.event_id, event.product_id, event.event_type,
                   product.event_id AS current_event_id,
                   product.title_text, product.title_language,
                   product.description_text, product.description_language,
                   product.product_images
            FROM product_events event
            JOIN products product ON product.product_id = event.product_id
            WHERE event.event_id = $1 AND event.product_id = $2
        "#,
        )
        .bind(uuid::Uuid::from(event_id))
        .bind(uuid::Uuid::from(product_id))
        .fetch_optional(&self.pool)
        .await
        .map_err(|source| ProductEmbeddingSourceReadError::QueryFailed {
            source: box_error(ProductEmbeddingSourceQueryError(source)),
        })?;
        row.map(TryInto::try_into).transpose()
    }
}

impl TryFrom<ProductEmbeddingSourceRow> for ProductEmbeddingSource {
    type Error = ProductEmbeddingSourceReadError;

    fn try_from(row: ProductEmbeddingSourceRow) -> Result<Self, Self::Error> {
        Ok(Self {
            product_id: ProductId::from(row.product_id),
            event_id: EventId::from(row.event_id),
            current_event_id: EventId::from(row.current_event_id),
            event_type: row.event_type,
            title: localized_title(row.title_text, row.title_language)?,
            description: localized_description(row.description_text, row.description_language)?,
            image_url: first_image_url(row.product_images)?,
        })
    }
}

fn localized_title(
    text: Option<String>,
    language: Option<String>,
) -> Result<Option<Localized<Language, Title>>, ProductEmbeddingSourceReadError> {
    match (text, language) {
        (Some(raw_text), Some(raw_language)) => {
            let title = Title::from(raw_text.as_str());
            if title.as_ref().is_empty() || title.as_ref() != raw_text {
                return Err(mapping_error(
                    "persisted product embedding title is invalid",
                ));
            }
            Ok(Some(Localized::new(parse_language(&raw_language)?, title)))
        }
        (None, None) => Ok(None),
        _ => Err(mapping_error(
            "persisted product embedding title is incomplete",
        )),
    }
}

fn localized_description(
    text: Option<String>,
    language: Option<String>,
) -> Result<Option<Localized<Language, Description>>, ProductEmbeddingSourceReadError> {
    match (text, language) {
        (Some(raw_text), Some(raw_language)) => {
            let description = Description::from(raw_text.as_str());
            if description.as_ref().is_empty() || description.as_ref() != raw_text {
                return Err(mapping_error(
                    "persisted product embedding description is invalid",
                ));
            }
            Ok(Some(Localized::new(
                parse_language(&raw_language)?,
                description,
            )))
        }
        (None, None) => Ok(None),
        _ => Err(mapping_error(
            "persisted product embedding description is incomplete",
        )),
    }
}

fn first_image_url(
    value: serde_json::Value,
) -> Result<Option<Url>, ProductEmbeddingSourceReadError> {
    let images: Vec<ProductImageRecord> = serde_json::from_value(value)
        .map_err(|_| mapping_error("persisted product embedding images are invalid"))?;
    images
        .into_iter()
        .next()
        .map(|image| {
            let url = Url::parse(&image.url)
                .map_err(|_| mapping_error("persisted product embedding image URL is invalid"))?;
            if !matches!(url.scheme(), "http" | "https") {
                return Err(mapping_error(
                    "persisted product embedding image URL is invalid",
                ));
            }
            Ok(url)
        })
        .transpose()
}

fn parse_language(value: &str) -> Result<Language, ProductEmbeddingSourceReadError> {
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
            "persisted product embedding language is invalid",
        )),
    }
}

fn mapping_error(message: &'static str) -> ProductEmbeddingSourceReadError {
    ProductEmbeddingSourceReadError::InvalidPersistedState {
        source: box_error(ProductEmbeddingSourceMappingError {
            source: static_error(message),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_reject_noncanonical_embedding_source_language() {
        assert!(parse_language("EN").is_err());
    }
}
