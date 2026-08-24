use crate::url::append_utm_params;
use application::error::{BoxError, box_error, static_error};
use domain_primitives::event_id::EventId;
use fxrate_core::FxRateId;
use geo::core::address::{GeoAddress, StructuredAddress};
use indexmap::IndexSet;
use localization::{Language, Localized};
use money::{Currency, MonetaryAmount, Price};
use platform_postgres::SqlxTransaction;
use product_core::{
    description::Description,
    product::{ProductAddress, ProductAuction, ProductPricing, ProductSaleValuation},
    product_id::ProductId,
    product_image::ProductImage,
    product_lifecycle::ProductLifecycle,
    product_slug_id::ProductSlugId,
    product_state::ProductState,
    prohibited_content::ProhibitedContent,
    shops_product_id::ShopsProductId,
    title::Title,
};
use product_service::ports::{
    ProductSearchFilterMatchShopType, ProductSearchFilterMatchSource,
    ProductSearchFilterMatchSourceEventKind, ProductSearchFilterMatchSourceReadError,
    ProductSearchFilterMatchSourceReader, ProductSearchFilterMatchSourceReaderFactory,
    ProductSearchFilterMatchSourceRef,
};
use shop_core::seller_slug_id::SellerSlugId;
use shop_core::shop_id::ShopId;
use shop_core::shop_name::ShopName;
use shop_core::shop_slug_id::ShopSlugId;
use shop_core::shop_type::ShopType;
use sqlx::PgConnection;
use std::collections::HashMap;

use time::OffsetDateTime;
use url::Url;

#[derive(Debug, Clone, Copy, Default)]
pub struct SqlxProductSearchFilterMatchSourceReaderFactory;

struct SqlxProductSearchFilterMatchSourceReader<'tx> {
    connection: &'tx mut PgConnection,
}

#[derive(Debug, Clone, sqlx::FromRow)]
struct SourceRow {
    event_id: uuid::Uuid,
    event_group: String,
    origin_event_time: OffsetDateTime,
    current_event_id: uuid::Uuid,
    projection_version: i64,
    product_id: uuid::Uuid,
    product_slug_id: String,
    shop_id: uuid::Uuid,
    shop_slug_id: String,
    shop_name: String,
    shop_type: String,
    seller_id: uuid::Uuid,
    seller_slug_id: String,
    seller_name: String,
    shops_product_id: String,
    structured_address_addressline: Option<String>,
    structured_address_addressline_extra: Option<String>,
    structured_address_locality: Option<String>,
    structured_address_region: Option<String>,
    structured_address_postal_code: Option<String>,
    structured_address_country: Option<String>,
    geo_address_lat: Option<f64>,
    geo_address_lon: Option<f64>,
    product_title_text: Option<String>,
    product_title_language: Option<String>,
    product_description_text: Option<String>,
    product_description_language: Option<String>,
    price_amount: Option<i64>,
    price_currency: Option<String>,
    price_estimate_min_amount: Option<i64>,
    price_estimate_min_currency: Option<String>,
    price_estimate_max_amount: Option<i64>,
    price_estimate_max_currency: Option<String>,
    sale_fx_rate_id: Option<uuid::Uuid>,
    sold_at: Option<OffsetDateTime>,
    state: String,
    lifecycle: String,
    url: String,
    product_images: serde_json::Value,
    embedding: Option<Vec<f32>>,
    auction_start: Option<OffsetDateTime>,
    auction_end: Option<OffsetDateTime>,
    created: OffsetDateTime,
    updated: OffsetDateTime,
    translation_language: Option<String>,
    translation_title: Option<String>,
    translation_description: Option<String>,
}

#[derive(Debug, thiserror::Error)]
#[error("product search-filter match source SQL query failed")]
struct SourceQuerySqlxError(#[source] sqlx::Error);

#[derive(Debug, thiserror::Error)]
#[error("product search-filter match source row is invalid")]
struct SourceRowMappingError {
    #[source]
    source: BoxError,
}

impl SourceRowMappingError {
    fn invalid(message: &'static str) -> Self {
        Self {
            source: static_error(message),
        }
    }
}

impl From<SourceQuerySqlxError> for ProductSearchFilterMatchSourceReadError {
    fn from(source: SourceQuerySqlxError) -> Self {
        Self::QueryFailed {
            source: box_error(source),
        }
    }
}

impl From<SourceRowMappingError> for ProductSearchFilterMatchSourceReadError {
    fn from(source: SourceRowMappingError) -> Self {
        Self::InvalidPersistedState {
            source: box_error(source),
        }
    }
}

impl SqlxProductSearchFilterMatchSourceReaderFactory {
    pub fn new() -> Self {
        Self
    }
}

impl ProductSearchFilterMatchSourceReaderFactory<SqlxTransaction>
    for SqlxProductSearchFilterMatchSourceReaderFactory
{
    fn in_transaction<'tx>(
        &'tx self,
        tx: &'tx mut SqlxTransaction,
    ) -> impl ProductSearchFilterMatchSourceReader + 'tx {
        SqlxProductSearchFilterMatchSourceReader {
            connection: tx.connection(),
        }
    }
}

#[async_trait::async_trait]
impl ProductSearchFilterMatchSourceReader for SqlxProductSearchFilterMatchSourceReader<'_> {
    async fn find_source(
        &mut self,
        event_id: EventId,
        product_id: ProductId,
    ) -> Result<Option<ProductSearchFilterMatchSource>, ProductSearchFilterMatchSourceReadError>
    {
        let reference = ProductSearchFilterMatchSourceRef {
            product_id,
            event_id,
        };
        Ok(self.find_sources(&[reference]).await?.remove(&reference))
    }

    async fn find_sources(
        &mut self,
        refs: &[ProductSearchFilterMatchSourceRef],
    ) -> Result<
        HashMap<ProductSearchFilterMatchSourceRef, ProductSearchFilterMatchSource>,
        ProductSearchFilterMatchSourceReadError,
    > {
        if refs.is_empty() {
            return Ok(HashMap::new());
        }

        let product_ids = refs
            .iter()
            .map(|reference| uuid::Uuid::from(reference.product_id))
            .collect::<Vec<_>>();
        let event_ids = refs
            .iter()
            .map(|reference| uuid::Uuid::from(reference.event_id))
            .collect::<Vec<_>>();
        let rows = sqlx::query_as::<_, SourceRow>(
            r#"
            WITH requested_events AS (
                SELECT DISTINCT product_id, event_id
                FROM UNNEST($1::uuid[], $2::uuid[]) AS requested(product_id, event_id)
            )
            SELECT
                event.event_id,
                event.event_group,
                event.event_time AS origin_event_time,
                product.event_id AS current_event_id,
                product.projection_version,
                product.product_id,
                product.product_slug_id,
                shop.shop_id,
                shop.shop_slug_id,
                shop.name AS shop_name,
                shop.shop_type,
                seller.shop_id AS seller_id,
                seller.shop_slug_id AS seller_slug_id,
                seller.name AS seller_name,
                product.shops_product_id,
                product.structured_address_addressline,
                product.structured_address_addressline_extra,
                product.structured_address_locality,
                product.structured_address_region,
                product.structured_address_postal_code,
                product.structured_address_country,
                product.geo_address_lat,
                product.geo_address_lon,
                product.title_text AS product_title_text,
                product.title_language AS product_title_language,
                product.description_text AS product_description_text,
                product.description_language AS product_description_language,
                product.price_amount,
                product.price_currency,
                product.price_estimate_min_amount,
                product.price_estimate_min_currency,
                product.price_estimate_max_amount,
                product.price_estimate_max_currency,
                product.sale_fx_rate_id,
                product.sold_at,
                product.state,
                product.lifecycle,
                product.url,
                product.product_images,
                product.embedding,
                product.auction_start,
                product.auction_end,
                product.created,
                product.updated,
                translation.language AS translation_language,
                translation.title AS translation_title,
                translation.description AS translation_description
            FROM requested_events requested
            JOIN product_events event
              ON event.product_id = requested.product_id
             AND event.event_id = requested.event_id
            JOIN products product ON product.product_id = event.product_id
            JOIN shops shop ON shop.shop_id = product.shop_id
            JOIN shops seller ON seller.shop_id = product.seller_id
            LEFT JOIN product_translations translation ON translation.product_id = product.product_id
            ORDER BY event.product_id ASC, event.event_id ASC, translation.language ASC
            "#,
        )
        .bind(product_ids)
        .bind(event_ids)
        .fetch_all(&mut *self.connection)
        .await
        .map_err(SourceQuerySqlxError)?;

        sources_from_rows(rows)
            .map_err(|_| {
                SourceRowMappingError::invalid(
                    "persisted product search-filter match source row is invalid",
                )
            })
            .map_err(Into::into)
    }
}

fn sources_from_rows(
    rows: Vec<SourceRow>,
) -> Result<HashMap<ProductSearchFilterMatchSourceRef, ProductSearchFilterMatchSource>, ()> {
    let mut grouped_rows = HashMap::<ProductSearchFilterMatchSourceRef, Vec<SourceRow>>::new();
    for row in rows {
        let reference = ProductSearchFilterMatchSourceRef {
            product_id: ProductId::from(row.product_id),
            event_id: EventId::from(row.event_id),
        };
        grouped_rows.entry(reference).or_default().push(row);
    }

    grouped_rows
        .into_iter()
        .map(|(reference, rows)| {
            let source = source_from_rows(rows)?.ok_or(())?;
            Ok((reference, source))
        })
        .collect()
}

fn source_from_rows(rows: Vec<SourceRow>) -> Result<Option<ProductSearchFilterMatchSource>, ()> {
    let Some(row) = rows.first() else {
        return Ok(None);
    };

    let product_title = localized_title(
        row.product_title_text.as_deref(),
        row.product_title_language.as_deref(),
    )?;
    let product_description = localized_description(
        row.product_description_text.as_deref(),
        row.product_description_language.as_deref(),
    )?;
    let (titles, descriptions) =
        translations(&rows, product_title.as_ref(), product_description.as_ref())?;
    let pricing = ProductPricing {
        price: price(row.price_amount, row.price_currency.as_deref())?,
        price_estimate_min: price(
            row.price_estimate_min_amount,
            row.price_estimate_min_currency.as_deref(),
        )?,
        price_estimate_max: price(
            row.price_estimate_max_amount,
            row.price_estimate_max_currency.as_deref(),
        )?,
    };
    let sale_valuation = sale_valuation(row.sale_fx_rate_id, row.sold_at)?;
    let images = images(&row.product_images)?;
    let url = Url::parse(&row.url).map_err(|_| ())?;
    let shop_slug_id = ShopSlugId::raw(&row.shop_slug_id).map_err(|_| ())?;

    Ok(Some(ProductSearchFilterMatchSource {
        event_id: EventId::from(row.event_id),
        event_kind: event_kind(&row.event_group),
        origin_event_time: row.origin_event_time,
        current_event_id: EventId::from(row.current_event_id),
        projection_version: row.projection_version,
        product_id: ProductId::from(row.product_id),
        product_slug_id: ProductSlugId::raw(&row.product_slug_id).map_err(|_| ())?,
        shop_id: ShopId::from(row.shop_id),
        shop_slug_id,
        shop_name: ShopName::from(row.shop_name.clone()),
        shop_type: shop_type(&row.shop_type)?,
        seller_id: ShopId::from(row.seller_id),
        seller_slug_id: SellerSlugId::from(ShopSlugId::raw(&row.seller_slug_id).map_err(|_| ())?),
        seller_name: ShopName::from(row.seller_name.clone()),
        shops_product_id: ShopsProductId::from(row.shops_product_id.clone()),
        address: address(row)?,
        product_title,
        product_description,
        titles,
        descriptions,
        pricing,
        sale_valuation,
        state: product_state(&row.state)?,
        lifecycle: lifecycle(&row.lifecycle)?,
        view_url: append_utm_params(url.clone()),
        url,
        image: images.iter().next().cloned(),
        images,
        embedding: row.embedding.clone(),
        auction: ProductAuction {
            start: row.auction_start,
            end: row.auction_end,
        },
        created: row.created,
        updated: row.updated,
    }))
}

type LocalizedTexts = (HashMap<Language, Title>, HashMap<Language, Description>);

fn translations(
    rows: &[SourceRow],
    product_title: Option<&Localized<Language, Title>>,
    product_description: Option<&Localized<Language, Description>>,
) -> Result<LocalizedTexts, ()> {
    let mut titles = HashMap::new();
    let mut descriptions = HashMap::new();

    for row in rows {
        match (
            row.translation_language.as_deref(),
            row.translation_title.as_deref(),
            row.translation_description.as_deref(),
        ) {
            (None, None, None) => {}
            (Some(language_value), Some(title_value), description_value) => {
                let language = language(language_value)?;
                titles.insert(language, title(title_value)?);
                if let Some(text) = description_value {
                    descriptions.insert(language, description(text)?);
                }
            }
            (Some(language_value), None, Some(description_value)) => {
                descriptions.insert(language(language_value)?, description(description_value)?);
            }
            _ => return Err(()),
        }
    }

    if let Some(title) = product_title {
        titles
            .entry(title.localization)
            .or_insert_with(|| title.payload.clone());
    }
    if let Some(description) = product_description {
        descriptions
            .entry(description.localization)
            .or_insert_with(|| description.payload.clone());
    }

    Ok((titles, descriptions))
}

fn address(row: &SourceRow) -> Result<ProductAddress, ()> {
    let structured = match row.structured_address_addressline.as_deref() {
        Some(addressline) => {
            let country = row
                .structured_address_country
                .as_deref()
                .map(isocountry::CountryCode::for_alpha3)
                .transpose()
                .map_err(|_| ())?;
            Some(StructuredAddress {
                addressline: Some(addressline.to_owned()),
                addressline_extra: row.structured_address_addressline_extra.clone(),
                locality: row.structured_address_locality.clone(),
                region: row.structured_address_region.clone(),
                postal_code: row.structured_address_postal_code.clone(),
                country,
                continent: country.map(geo::core::continent::Continent::from),
            })
        }
        None if row.structured_address_addressline_extra.is_none()
            && row.structured_address_locality.is_none()
            && row.structured_address_region.is_none()
            && row.structured_address_postal_code.is_none()
            && row.structured_address_country.is_none() =>
        {
            None
        }
        None => return Err(()),
    };
    let geo = match (row.geo_address_lat, row.geo_address_lon) {
        (Some(lat), Some(lon))
            if (-90.0..=90.0).contains(&lat) && (-180.0..=180.0).contains(&lon) =>
        {
            Some(GeoAddress { lat, lon })
        }
        (None, None) => None,
        _ => return Err(()),
    };

    Ok(ProductAddress { structured, geo })
}

fn localized_title(
    text: Option<&str>,
    language_value: Option<&str>,
) -> Result<Option<Localized<Language, Title>>, ()> {
    match (text, language_value) {
        (Some(text), Some(language_value)) => Ok(Some(Localized::new(
            language(language_value)?,
            title(text)?,
        ))),
        (None, None) => Ok(None),
        _ => Err(()),
    }
}

fn localized_description(
    text: Option<&str>,
    language_value: Option<&str>,
) -> Result<Option<Localized<Language, Description>>, ()> {
    match (text, language_value) {
        (Some(text), Some(language_value)) => Ok(Some(Localized::new(
            language(language_value)?,
            description(text)?,
        ))),
        (None, None) => Ok(None),
        _ => Err(()),
    }
}

fn title(value: &str) -> Result<Title, ()> {
    let title = Title::from(value);
    (!title.as_ref().is_empty() && title.as_ref() == value)
        .then_some(title)
        .ok_or(())
}

fn description(value: &str) -> Result<Description, ()> {
    let description = Description::from(value);
    (!description.as_ref().is_empty() && description.as_ref() == value)
        .then_some(description)
        .ok_or(())
}

fn price(amount: Option<i64>, currency_value: Option<&str>) -> Result<Option<Price>, ()> {
    match (amount, currency_value) {
        (Some(amount), Some(currency_value)) => Ok(Some(Price::new(
            MonetaryAmount::from(u64::try_from(amount).map_err(|_| ())?),
            currency(currency_value)?,
        ))),
        (None, None) => Ok(None),
        _ => Err(()),
    }
}

fn sale_valuation(
    fx_rate_id: Option<uuid::Uuid>,
    sold_at: Option<OffsetDateTime>,
) -> Result<Option<ProductSaleValuation>, ()> {
    match (fx_rate_id, sold_at) {
        (Some(fx_rate_id), Some(sold_at)) => Ok(Some(ProductSaleValuation {
            fx_rate_id: FxRateId::from(fx_rate_id),
            sold_at,
        })),
        (None, None) => Ok(None),
        _ => Err(()),
    }
}

fn images(value: &serde_json::Value) -> Result<IndexSet<ProductImage>, ()> {
    #[derive(serde::Deserialize)]
    struct ImageJson {
        url: String,
        prohibited_content: String,
    }

    serde_json::from_value::<Vec<ImageJson>>(value.clone())
        .map_err(|_| ())?
        .into_iter()
        .map(|image| {
            Ok(ProductImage {
                url: Url::parse(&image.url).map_err(|_| ())?,
                prohibited_content: ProhibitedContent::from_code(&image.prohibited_content)
                    .ok_or(())?,
            })
        })
        .collect()
}

fn language(value: &str) -> Result<Language, ()> {
    Language::from_code(value).ok_or(())
}

fn currency(value: &str) -> Result<Currency, ()> {
    Currency::from_code(value).ok_or(())
}

fn product_state(value: &str) -> Result<ProductState, ()> {
    ProductState::from_code(value).ok_or(())
}

fn lifecycle(value: &str) -> Result<ProductLifecycle, ()> {
    ProductLifecycle::from_code(value).ok_or(())
}

fn event_kind(value: &str) -> ProductSearchFilterMatchSourceEventKind {
    match value {
        "DOMAIN" => ProductSearchFilterMatchSourceEventKind::Domain,
        "ENRICHMENT" => ProductSearchFilterMatchSourceEventKind::Enrichment,
        _ => ProductSearchFilterMatchSourceEventKind::Ignored,
    }
}

fn shop_type(value: &str) -> Result<ProductSearchFilterMatchShopType, ()> {
    ShopType::from_code(value)
        .map(|shop_type| match shop_type {
            ShopType::AuctionHouse => ProductSearchFilterMatchShopType::AuctionHouse,
            ShopType::AuctionPlatform => ProductSearchFilterMatchShopType::AuctionPlatform,
            ShopType::CommercialDealer => ProductSearchFilterMatchShopType::CommercialDealer,
            ShopType::Marketplace => ProductSearchFilterMatchShopType::Marketplace,
        })
        .ok_or(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_preserve_sqlx_query_source() {
        let error: ProductSearchFilterMatchSourceReadError =
            SourceQuerySqlxError(sqlx::Error::RowNotFound).into();

        let ProductSearchFilterMatchSourceReadError::QueryFailed { source } = error else {
            panic!("expected source query failure");
        };
        assert!(source.downcast_ref::<SourceQuerySqlxError>().is_some());
        assert!(source.source().is_some());
    }

    #[test]
    fn should_preserve_row_mapping_source() {
        let error: ProductSearchFilterMatchSourceReadError =
            SourceRowMappingError::invalid("invalid persisted state").into();

        let ProductSearchFilterMatchSourceReadError::InvalidPersistedState { source } = error
        else {
            panic!("expected invalid persisted state");
        };
        let mapping_error = source
            .downcast_ref::<SourceRowMappingError>()
            .unwrap_or_else(|| panic!("expected source row mapping error"));
        assert!(std::error::Error::source(mapping_error).is_some());
    }

    #[test]
    fn should_classify_percolation_event_groups() {
        assert_eq!(
            ProductSearchFilterMatchSourceEventKind::Domain,
            event_kind("DOMAIN")
        );
        assert_eq!(
            ProductSearchFilterMatchSourceEventKind::Enrichment,
            event_kind("ENRICHMENT")
        );
        assert_eq!(
            ProductSearchFilterMatchSourceEventKind::Ignored,
            event_kind("POLICY")
        );
    }

    #[test]
    fn should_map_sale_valuation_only_when_both_persisted_columns_are_present() {
        assert_eq!(Ok(None), sale_valuation(None, None));

        let fx_rate_id = uuid::Uuid::new_v4();
        let sold_at = OffsetDateTime::UNIX_EPOCH;
        assert_eq!(
            Ok(Some(ProductSaleValuation {
                fx_rate_id: FxRateId::from(fx_rate_id),
                sold_at,
            })),
            sale_valuation(Some(fx_rate_id), Some(sold_at))
        );

        assert!(sale_valuation(Some(uuid::Uuid::new_v4()), None).is_err());
        assert!(sale_valuation(None, Some(sold_at)).is_err());
    }

    #[test]
    fn should_reject_noncanonical_persisted_values() {
        assert!(language("EN").is_err());
        assert!(currency("eur").is_err());
        assert!(product_state("available").is_err());
        assert!(lifecycle("active").is_err());
        assert!(shop_type("commercial_dealer").is_err());
    }

    #[test]
    fn should_reject_noncanonical_localized_text() {
        assert!(title(" title ").is_err());
        assert!(description(" ").is_err());
    }
}
