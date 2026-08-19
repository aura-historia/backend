use localization::Language;
use std::collections::HashMap;

use common::{
    error::boxed::{BoxError, box_error, static_error},
    event_id::EventId,
    product_id::ProductId,
    product_slug_id::ProductSlugId,
    shops_product_id::ShopsProductId,
    utm::append_utm_params,
};
use platform_postgres::SqlxTransaction;
use product_core::title::Title;
use product_service::ports::{
    ProductWatchlistNotificationChange, ProductWatchlistNotificationSource,
    ProductWatchlistNotificationSourceReadError, ProductWatchlistNotificationSourceReader,
    ProductWatchlistNotificationSourceReaderFactory,
};
use product_service::use_cases::ProductEventPayload;
use shop_core::shop_id::ShopId;
use shop_core::shop_name::ShopName;
use shop_core::shop_slug_id::ShopSlugId;
use sqlx::PgConnection;

use super::product_details_reader::images;
use super::product_event_reader::parse_payload;

#[derive(Debug, Clone, Copy, Default)]
pub struct SqlxProductWatchlistNotificationSourceReaderFactory;

struct SqlxProductWatchlistNotificationSourceReader<'tx> {
    connection: &'tx mut PgConnection,
}

#[derive(Debug, sqlx::FromRow)]
struct SourceRow {
    event_id: uuid::Uuid,
    product_id: uuid::Uuid,
    event_type: String,
    payload: serde_json::Value,
    product_slug_id: String,
    shop_id: uuid::Uuid,
    shops_product_id: String,
    shop_slug_id: String,
    shop_name: String,
    title_text: Option<String>,
    title_language: Option<String>,
    product_images: serde_json::Value,
    url: String,
}

#[derive(Debug, sqlx::FromRow)]
struct TitleRow {
    language: String,
    title: String,
}

#[derive(Debug, thiserror::Error)]
#[error("watchlist notification source SQL query failed")]
struct WatchlistNotificationSourceQueryError(#[source] sqlx::Error);

#[derive(Debug, thiserror::Error)]
#[error("watchlist notification source persisted state could not be mapped")]
struct WatchlistNotificationSourceMappingError {
    #[source]
    source: BoxError,
}

impl WatchlistNotificationSourceMappingError {
    fn invalid(message: &'static str) -> Self {
        Self {
            source: static_error(message),
        }
    }

    fn with_source(source: impl std::error::Error + Send + Sync + 'static) -> Self {
        Self {
            source: box_error(source),
        }
    }
}

impl From<WatchlistNotificationSourceQueryError> for ProductWatchlistNotificationSourceReadError {
    fn from(source: WatchlistNotificationSourceQueryError) -> Self {
        Self::QueryFailed {
            source: box_error(source),
        }
    }
}

impl From<WatchlistNotificationSourceMappingError> for ProductWatchlistNotificationSourceReadError {
    fn from(source: WatchlistNotificationSourceMappingError) -> Self {
        Self::InvalidPersistedState {
            source: box_error(source),
        }
    }
}

impl SqlxProductWatchlistNotificationSourceReaderFactory {
    pub fn new() -> Self {
        Self
    }
}

impl ProductWatchlistNotificationSourceReaderFactory<SqlxTransaction>
    for SqlxProductWatchlistNotificationSourceReaderFactory
{
    fn in_transaction<'tx>(
        &'tx self,
        tx: &'tx mut SqlxTransaction,
    ) -> impl ProductWatchlistNotificationSourceReader + 'tx {
        SqlxProductWatchlistNotificationSourceReader {
            connection: tx.connection(),
        }
    }
}

#[async_trait::async_trait]
impl ProductWatchlistNotificationSourceReader for SqlxProductWatchlistNotificationSourceReader<'_> {
    async fn find_source(
        &mut self,
        event_id: EventId,
        product_id: ProductId,
    ) -> Result<
        Option<ProductWatchlistNotificationSource>,
        ProductWatchlistNotificationSourceReadError,
    > {
        let row = sqlx::query_as::<_, SourceRow>(
            r#"
            SELECT
                event.event_id, event.product_id, event.event_type, event.payload,
                product.product_slug_id, product.shop_id, product.shops_product_id,
                shop.shop_slug_id, shop.name AS shop_name,
                product.title_text, product.title_language, product.product_images, product.url
            FROM product_events event
            JOIN products product ON product.product_id = event.product_id
            JOIN shops shop ON shop.shop_id = product.shop_id
            WHERE event.event_id = $1 AND event.product_id = $2
            "#,
        )
        .bind(uuid::Uuid::from(event_id))
        .bind(uuid::Uuid::from(product_id))
        .fetch_optional(&mut *self.connection)
        .await
        .map_err(WatchlistNotificationSourceQueryError)?;
        let Some(row) = row else {
            return Ok(None);
        };
        let (_, payload) = parse_payload(&row.event_type, &row.payload)
            .map_err(WatchlistNotificationSourceMappingError::with_source)?;
        let change = match payload {
            ProductEventPayload::PriceChanged(change) => {
                ProductWatchlistNotificationChange::PriceChanged {
                    old_price: change.old_pricing.price,
                    new_price: change.new_pricing.price,
                }
            }
            ProductEventPayload::StateChanged(change) => {
                ProductWatchlistNotificationChange::StateChanged {
                    old_state: change.old_state,
                    new_state: change.new_state,
                }
            }
            _ => return Ok(None),
        };
        let translations = sqlx::query_as::<_, TitleRow>(
            "SELECT language, title FROM product_translations WHERE product_id = $1 AND title IS NOT NULL",
        )
        .bind(row.product_id)
        .fetch_all(&mut *self.connection)
        .await
        .map_err(WatchlistNotificationSourceQueryError)?;
        let mut title = HashMap::new();
        if let (Some(language), Some(text)) = (row.title_language.as_deref(), row.title_text) {
            title.insert(parse_language(language)?, Title::from(text));
        }
        for translation in translations {
            title.insert(
                parse_language(&translation.language)?,
                Title::from(translation.title),
            );
        }
        let image = images(row.product_images)
            .map_err(|_| {
                WatchlistNotificationSourceMappingError::invalid(
                    "persisted watchlist notification source images are invalid",
                )
            })?
            .into_iter()
            .next();
        let url = url::Url::parse(&row.url)
            .map_err(WatchlistNotificationSourceMappingError::with_source)?;
        Ok(Some(ProductWatchlistNotificationSource {
            event_id: EventId::from(row.event_id),
            product_id: ProductId::from(row.product_id),
            product_slug_id: ProductSlugId::raw(&row.product_slug_id)
                .map_err(WatchlistNotificationSourceMappingError::with_source)?,
            shop_id: ShopId::from(row.shop_id),
            shops_product_id: ShopsProductId::from(row.shops_product_id),
            shop_slug_id: ShopSlugId::raw(&row.shop_slug_id)
                .map_err(WatchlistNotificationSourceMappingError::with_source)?,
            shop_name: ShopName::from(row.shop_name),
            title: (!title.is_empty()).then_some(title),
            image,
            view_url: append_utm_params(url.clone()),
            url,
            change,
        }))
    }
}

fn parse_language(value: &str) -> Result<Language, ProductWatchlistNotificationSourceReadError> {
    match value.to_ascii_lowercase().as_str() {
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
        _ => Err(WatchlistNotificationSourceMappingError::invalid(
            "persisted watchlist notification source language is invalid",
        )
        .into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_preserve_sqlx_query_source() {
        let error: ProductWatchlistNotificationSourceReadError =
            WatchlistNotificationSourceQueryError(sqlx::Error::RowNotFound).into();

        let ProductWatchlistNotificationSourceReadError::QueryFailed { source } = error else {
            panic!("expected source query failure");
        };
        let query_error = source
            .downcast_ref::<WatchlistNotificationSourceQueryError>()
            .unwrap_or_else(|| panic!("expected watchlist notification query error"));
        assert!(std::error::Error::source(query_error).is_some());
    }

    #[test]
    fn should_preserve_persisted_state_mapping_source() {
        let error: ProductWatchlistNotificationSourceReadError =
            WatchlistNotificationSourceMappingError::invalid("invalid persisted state").into();

        let ProductWatchlistNotificationSourceReadError::InvalidPersistedState { source } = error
        else {
            panic!("expected invalid persisted state");
        };
        let mapping_error = source
            .downcast_ref::<WatchlistNotificationSourceMappingError>()
            .unwrap_or_else(|| panic!("expected watchlist notification mapping error"));
        assert!(std::error::Error::source(mapping_error).is_some());
    }
}
