use common::currency::domain::Currency;
use common::event_id::EventId;
use common::language::domain::Language;
use common::localized::Localized;
use common::price::domain::{MonetaryAmount, Price};
use common::product_id::ProductId;
use common::product_lifecycle::domain::ProductLifecycle;
use common::product_slug_id::ProductSlugId;
use common::product_state::domain::ProductState;
use common::shop_id::ShopId;
use common::shop_name::ShopName;
use common::shop_slug_id::ShopSlugId;
use common::shops_product_id::ShopsProductId;
use common::utm::append_utm_params;
use indexmap::IndexSet;
use product_core::description::Description;
use product_core::fx_rate_id::FxRateId;
use product_core::product::{ProductAddress, ProductAuction, ProductPricing};
use product_core::product_image::ProductImage;
use product_core::prohibited_content::ProhibitedContent;
use product_core::title::Title;
use product_service::ports::{
    ProductDetailsReadError, ProductDetailsReader, ProductDetailsReaderFactory,
};
use product_service::use_cases::queries::get_product::{
    GetProductRequest, ProductDetailsView, ProductLookup,
};
use serde::Deserialize;
use sqlx::PgConnection;
use time::OffsetDateTime;
use url::Url;

#[derive(Debug, Clone, Copy, Default)]
pub struct SqlxProductDetailsReaderFactory;

struct SqlxProductDetailsReader<'tx> {
    connection: &'tx mut PgConnection,
}

#[derive(Debug, sqlx::FromRow)]
struct ProductDetailsRow {
    product_id: uuid::Uuid,
    product_slug_id: String,
    event_id: uuid::Uuid,
    shop_id: uuid::Uuid,
    seller_id: uuid::Uuid,
    shops_product_id: String,
    shop_name: String,
    shop_slug_id: String,
    seller_name: String,
    seller_slug_id: String,
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
    title_text: Option<String>,
    title_language: Option<String>,
    description_text: Option<String>,
    description_language: Option<String>,
    price_amount: Option<i64>,
    price_currency: Option<String>,
    price_estimate_min_amount: Option<i64>,
    price_estimate_min_currency: Option<String>,
    price_estimate_max_amount: Option<i64>,
    price_estimate_max_currency: Option<String>,
    fx_rate_id: Option<uuid::Uuid>,
    state: String,
    lifecycle: String,
    url: String,
    product_images: serde_json::Value,
    auction_start: Option<OffsetDateTime>,
    auction_end: Option<OffsetDateTime>,
    created: OffsetDateTime,
    updated: OffsetDateTime,
}

#[derive(Debug, Deserialize)]
struct ProductImageJson {
    url: String,
    prohibited_content: String,
}

impl SqlxProductDetailsReaderFactory {
    pub fn new() -> Self {
        Self
    }
}

impl ProductDetailsReaderFactory<common::postgres::SqlxTransaction>
    for SqlxProductDetailsReaderFactory
{
    fn in_transaction<'tx>(
        &'tx self,
        tx: &'tx mut common::postgres::SqlxTransaction,
    ) -> impl ProductDetailsReader + 'tx {
        SqlxProductDetailsReader {
            connection: tx.connection(),
        }
    }
}

#[async_trait::async_trait]
impl ProductDetailsReader for SqlxProductDetailsReader<'_> {
    async fn find_details(
        &mut self,
        request: &GetProductRequest,
    ) -> Result<Option<ProductDetailsView>, ProductDetailsReadError> {
        let requested_language = request.language.as_str();
        let row = match &request.lookup {
            ProductLookup::ById(product_id) => {
                sqlx::query_as::<_, ProductDetailsRow>(&format!(
                    "{SELECT_PRODUCT_DETAILS} WHERE p.product_id = $2"
                ))
                .bind(requested_language)
                .bind(uuid::Uuid::from(*product_id))
                .fetch_optional(&mut *self.connection)
                .await
            }

            ProductLookup::BySlug {
                shop_slug_id,
                product_slug_id,
            } => sqlx::query_as::<_, ProductDetailsRow>(&format!(
                "{SELECT_PRODUCT_DETAILS} WHERE shop.shop_slug_id = $2 AND p.product_slug_id = $3"
            ))
            .bind(requested_language)
            .bind(shop_slug_id.as_ref())
            .bind(product_slug_id.as_ref())
            .fetch_optional(&mut *self.connection)
            .await,
        }
        .map_err(|_| ProductDetailsReadError::ProductDetailsQueryFailed)?;

        row.map(TryInto::try_into)
            .transpose()
            .map_err(|_| ProductDetailsReadError::ProductDetailsReadModelInvalid)
    }
}

const SELECT_PRODUCT_DETAILS: &str = r#"
    SELECT
        p.product_id, p.product_slug_id, p.event_id, p.shop_id, p.seller_id, p.shops_product_id,
        shop.name AS shop_name, shop.shop_slug_id,
        seller.name AS seller_name, seller.shop_slug_id AS seller_slug_id,
        p.structured_address_addressline, p.structured_address_addressline_extra,
        p.structured_address_locality, p.structured_address_region, p.structured_address_postal_code,
        p.structured_address_country, p.geo_address_lat, p.geo_address_lon,
        p.title_text AS product_title_text, p.title_language AS product_title_language,
        p.description_text AS product_description_text,
        p.description_language AS product_description_language,
        selected_text.title_text, selected_text.title_language,
        selected_text.description_text, selected_text.description_language,
        p.price_amount, p.price_currency, p.price_estimate_min_amount,
        p.price_estimate_min_currency, p.price_estimate_max_amount,
        p.price_estimate_max_currency, p.fx_rate_id, p.state, p.lifecycle, p.url,
        p.product_images, p.auction_start, p.auction_end, p.created, p.updated
    FROM products p
    JOIN shops shop ON shop.shop_id = p.shop_id
    JOIN shops seller ON seller.shop_id = p.seller_id
    LEFT JOIN LATERAL (
        SELECT
            (
                array_agg(
                    candidates.title_text
                    ORDER BY
                        CASE lower(candidates.title_language)
                            WHEN lower($1) THEN 0
                            WHEN 'en' THEN 1
                            WHEN 'de' THEN 2
                            ELSE 3
                        END,
                        lower(candidates.title_language),
                        candidates.source_priority,
                        candidates.title_language,
                        candidates.title_text
                ) FILTER (WHERE candidates.title_text IS NOT NULL AND candidates.title_language IS NOT NULL)
            )[1] AS title_text,
            (
                array_agg(
                    candidates.title_language
                    ORDER BY
                        CASE lower(candidates.title_language)
                            WHEN lower($1) THEN 0
                            WHEN 'en' THEN 1
                            WHEN 'de' THEN 2
                            ELSE 3
                        END,
                        lower(candidates.title_language),
                        candidates.source_priority,
                        candidates.title_language,
                        candidates.title_text
                ) FILTER (WHERE candidates.title_text IS NOT NULL AND candidates.title_language IS NOT NULL)
            )[1] AS title_language,
            (
                array_agg(
                    candidates.description_text
                    ORDER BY
                        CASE lower(candidates.description_language)
                            WHEN lower($1) THEN 0
                            WHEN 'en' THEN 1
                            WHEN 'de' THEN 2
                            ELSE 3
                        END,
                        lower(candidates.description_language),
                        candidates.source_priority,
                        candidates.description_language,
                        candidates.description_text
                ) FILTER (
                    WHERE candidates.description_text IS NOT NULL
                        AND candidates.description_language IS NOT NULL
                )
            )[1] AS description_text,
            (
                array_agg(
                    candidates.description_language
                    ORDER BY
                        CASE lower(candidates.description_language)
                            WHEN lower($1) THEN 0
                            WHEN 'en' THEN 1
                            WHEN 'de' THEN 2
                            ELSE 3
                        END,
                        lower(candidates.description_language),
                        candidates.source_priority,
                        candidates.description_language,
                        candidates.description_text
                ) FILTER (
                    WHERE candidates.description_text IS NOT NULL
                        AND candidates.description_language IS NOT NULL
                )
            )[1] AS description_language
        FROM (
            SELECT
                translation.title AS title_text,
                translation.language AS title_language,
                translation.description AS description_text,
                translation.language AS description_language,
                0 AS source_priority
            FROM product_translations translation
            WHERE translation.product_id = p.product_id

            UNION ALL

            SELECT
                p.title_text,
                p.title_language,
                p.description_text,
                p.description_language,
                1 AS source_priority
        ) AS candidates
    ) AS selected_text ON TRUE
"#;

impl TryFrom<ProductDetailsRow> for ProductDetailsView {
    type Error = ();

    fn try_from(row: ProductDetailsRow) -> Result<Self, Self::Error> {
        let address = address(&row)?;
        let product_title = localized_title(row.product_title_text, row.product_title_language)?;
        let product_description = localized_description(
            row.product_description_text,
            row.product_description_language,
        )?;
        let title = localized_title(row.title_text, row.title_language)?;
        let description = localized_description(row.description_text, row.description_language)?;
        let product_price = price(row.price_amount, row.price_currency)?;
        let product_price_estimate_min = price(
            row.price_estimate_min_amount,
            row.price_estimate_min_currency,
        )?;
        let product_price_estimate_max = price(
            row.price_estimate_max_amount,
            row.price_estimate_max_currency,
        )?;
        let url = Url::parse(&row.url).map_err(|_| ())?;
        let currency = product_price
            .or(product_price_estimate_min)
            .or(product_price_estimate_max)
            .map(|value| value.currency);

        Ok(ProductDetailsView {
            product_id: ProductId::from(row.product_id),
            product_slug_id: ProductSlugId::raw(&row.product_slug_id).map_err(|_| ())?,
            event_id: EventId::from(row.event_id),
            shop_id: ShopId::from(row.shop_id),
            seller_id: ShopId::from(row.seller_id),
            shops_product_id: ShopsProductId::from(row.shops_product_id),
            shop_name: ShopName::from(row.shop_name),
            seller_name: ShopName::from(row.seller_name),
            shop_slug_id: ShopSlugId::raw(&row.shop_slug_id).map_err(|_| ())?,
            seller_slug_id: ShopSlugId::raw(&row.seller_slug_id).map_err(|_| ())?,
            address,
            product_title,
            product_description,
            title,
            description,
            pricing: ProductPricing {
                price: product_price,
                price_estimate_min: product_price_estimate_min,
                price_estimate_max: product_price_estimate_max,
                fx_rate_id: row.fx_rate_id.map(FxRateId::from),
            },
            price: product_price,
            price_estimate_min: product_price_estimate_min,
            price_estimate_max: product_price_estimate_max,
            currency,
            state: product_state(&row.state)?,
            lifecycle: lifecycle(&row.lifecycle)?,
            view_url: append_utm_params(url.clone()),
            url,
            images: images(row.product_images)?,
            auction: ProductAuction {
                start: row.auction_start,
                end: row.auction_end,
            },
            created: row.created,
            updated: row.updated,
        })
    }
}

fn address(row: &ProductDetailsRow) -> Result<ProductAddress, ()> {
    let structured = match &row.structured_address_addressline {
        Some(addressline) => Some(geo::core::address::StructuredAddress {
            addressline: Some(addressline.clone()),
            addressline_extra: row.structured_address_addressline_extra.clone(),
            locality: row.structured_address_locality.clone(),
            region: row.structured_address_region.clone(),
            postal_code: row.structured_address_postal_code.clone(),
            country: row
                .structured_address_country
                .as_deref()
                .map(|value| isocountry::CountryCode::for_alpha3(value).map_err(|_| ()))
                .transpose()?,
            continent: row
                .structured_address_country
                .as_deref()
                .map(|value| {
                    isocountry::CountryCode::for_alpha3(value)
                        .map(geo::core::continent::Continent::from)
                        .map_err(|_| ())
                })
                .transpose()?,
        }),
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
        (Some(lat), Some(lon)) => Some(geo::core::address::GeoAddress { lat, lon }),
        (None, None) => None,
        _ => return Err(()),
    };
    Ok(ProductAddress { structured, geo })
}

fn localized_title(
    text: Option<String>,
    language: Option<String>,
) -> Result<Option<Localized<Language, Title>>, ()> {
    match (text, language) {
        (Some(text), Some(language)) => {
            let title = Title::from(text.as_str());
            if title.as_ref().is_empty() || title.as_ref() != text.as_str() {
                return Err(());
            }
            Ok(Some(Localized::new(parse_language(&language)?, title)))
        }
        (None, None) => Ok(None),
        _ => Err(()),
    }
}

fn localized_description(
    text: Option<String>,
    language: Option<String>,
) -> Result<Option<Localized<Language, Description>>, ()> {
    match (text, language) {
        (Some(text), Some(language)) => {
            let description = Description::from(text.as_str());
            if description.as_ref().is_empty() || description.as_ref() != text.as_str() {
                return Err(());
            }
            Ok(Some(Localized::new(
                parse_language(&language)?,
                description,
            )))
        }
        (None, None) => Ok(None),
        _ => Err(()),
    }
}

fn price(amount: Option<i64>, currency: Option<String>) -> Result<Option<Price>, ()> {
    match (amount, currency) {
        (Some(amount), Some(currency)) => Ok(Some(Price::new(
            MonetaryAmount::from(u64::try_from(amount).map_err(|_| ())?),
            parse_currency(&currency)?,
        ))),
        (None, None) => Ok(None),
        _ => Err(()),
    }
}

fn images(value: serde_json::Value) -> Result<IndexSet<ProductImage>, ()> {
    serde_json::from_value::<Vec<ProductImageJson>>(value)
        .map_err(|_| ())?
        .into_iter()
        .map(|image| {
            Ok(ProductImage {
                url: Url::parse(&image.url).map_err(|_| ())?,
                prohibited_content: match image.prohibited_content.as_str() {
                    "UNKNOWN" => ProhibitedContent::Unknown,
                    "NONE" => ProhibitedContent::None,
                    "NAZI_GERMANY" => ProhibitedContent::NaziGermany,
                    _ => return Err(()),
                },
            })
        })
        .collect()
}

fn parse_language(value: &str) -> Result<Language, ()> {
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
        _ => Err(()),
    }
}

fn parse_currency(value: &str) -> Result<Currency, ()> {
    match value.to_ascii_uppercase().as_str() {
        "EUR" => Ok(Currency::Eur),
        "GBP" => Ok(Currency::Gbp),
        "USD" => Ok(Currency::Usd),
        "AUD" => Ok(Currency::Aud),
        "CAD" => Ok(Currency::Cad),
        "NZD" => Ok(Currency::Nzd),
        "CNY" => Ok(Currency::Cny),
        "BRL" => Ok(Currency::Brl),
        "PLN" => Ok(Currency::Pln),
        "TRY" => Ok(Currency::Try),
        "JPY" => Ok(Currency::Jpy),
        "CZK" => Ok(Currency::Czk),
        "RUB" => Ok(Currency::Rub),
        "AED" => Ok(Currency::Aed),
        "SAR" => Ok(Currency::Sar),
        "HKD" => Ok(Currency::Hkd),
        "SGD" => Ok(Currency::Sgd),
        "CHF" => Ok(Currency::Chf),
        _ => Err(()),
    }
}

fn product_state(value: &str) -> Result<ProductState, ()> {
    match value {
        "LISTED" => Ok(ProductState::Listed),
        "AVAILABLE" => Ok(ProductState::Available),
        "RESERVED" => Ok(ProductState::Reserved),
        "SOLD" => Ok(ProductState::Sold),
        "REMOVED" => Ok(ProductState::Removed),
        "UNKNOWN" => Ok(ProductState::Unknown),
        _ => Err(()),
    }
}

fn lifecycle(value: &str) -> Result<ProductLifecycle, ()> {
    match value {
        "ACTIVE" => Ok(ProductLifecycle::Active),
        "DELETED" => Ok(ProductLifecycle::Deleted),
        _ => Err(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_reject_selected_title_when_text_is_not_canonical() {
        let result = localized_title(Some("title".to_owned()), Some("en".to_owned()));

        assert!(result.is_err());
    }

    #[test]
    fn should_reject_selected_description_when_text_is_empty() {
        let result = localized_description(Some(" ".to_owned()), Some("en".to_owned()));

        assert!(result.is_err());
    }

    #[test]
    fn should_reject_selected_text_when_language_is_unrecognized() {
        let result = localized_title(Some("Title".to_owned()), Some("xx".to_owned()));

        assert!(result.is_err());
    }
}
