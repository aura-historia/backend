use common::currency::domain::Currency;
use common::enhanced_match_reason::EnhancedMatchReason;
use common::event_id::EventId;
use common::language::domain::Language;
use common::localized::Localized;
use common::personalized::Personalized;
use common::price::domain::{MonetaryAmount, Price};
use common::product_id::ProductId;
use common::product_lifecycle::domain::ProductLifecycle;
use common::product_slug_id::ProductSlugId;
use common::product_state::domain::ProductState;
use common::shop_id::ShopId;
use common::shop_name::ShopName;
use common::shop_slug_id::ShopSlugId;
use common::shops_product_id::ShopsProductId;
use common::user_search_filter_id::UserSearchFilterId;
use common::user_search_filter_name::UserSearchFilterName;
use common::utm::append_utm_params;
use indexmap::IndexSet;
use product_core::description::Description;
use product_core::product::{ProductAddress, ProductAuction, ProductPricing};
use product_core::product_image::ProductImage;
use product_core::prohibited_content::ProhibitedContent;
use product_core::title::Title;
use product_core::user_state::{
    ProductUserState, ProhibitedContentUserState, SearchFilterUserState, WatchlistUserState,
};
use product_service::ports::{
    ProductDetailsReadError, ProductDetailsReadRequest, ProductDetailsReader,
    ProductDetailsReaderFactory,
};
use product_service::use_cases::queries::get_product::{
    PersonalizedProductDetailsView, ProductDetailsView, ProductLookup,
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
pub(super) struct ProductDetailsRow {
    pub(super) product_id: uuid::Uuid,
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
    state: String,
    lifecycle: String,
    url: String,
    product_images: serde_json::Value,
    auction_start: Option<OffsetDateTime>,
    auction_end: Option<OffsetDateTime>,
    created: OffsetDateTime,
    updated: OffsetDateTime,
    personalization_user_id: Option<uuid::Uuid>,
    user_prohibited_content_consent: Option<bool>,
    user_tier: Option<String>,
    watchlist_notifications: Option<bool>,
    selected_match_user_search_filter_id: Option<uuid::Uuid>,
    selected_match_user_search_filter_name: Option<String>,
    selected_match_reason: Option<String>,
    selected_match_feedback: Option<bool>,
    selected_match_month_position: Option<i64>,
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
        request: &ProductDetailsReadRequest,
    ) -> Result<Option<PersonalizedProductDetailsView>, ProductDetailsReadError> {
        let requested_language = request.language.as_str();
        let user_id = request.user_id.map(uuid::Uuid::from);
        let row = match &request.lookup {
            ProductLookup::ById(product_id) => {
                sqlx::query_as::<_, ProductDetailsRow>(&format!(
                    "{SELECT_PRODUCT_DETAILS} WHERE p.product_id = $3"
                ))
                .bind(requested_language)
                .bind(user_id)
                .bind(uuid::Uuid::from(*product_id))
                .fetch_optional(&mut *self.connection)
                .await
            }

            ProductLookup::BySlug {
                shop_slug_id,
                product_slug_id,
            } => sqlx::query_as::<_, ProductDetailsRow>(&format!(
                "{SELECT_PRODUCT_DETAILS} WHERE shop.shop_slug_id = $3 AND p.product_slug_id = $4"
            ))
            .bind(requested_language)
            .bind(user_id)
            .bind(shop_slug_id.as_ref())
            .bind(product_slug_id.as_ref())
            .fetch_optional(&mut *self.connection)
            .await,
        }
        .map_err(|_| ProductDetailsReadError::ProductDetailsQueryFailed)?;

        row.map(PersonalizedProductDetailsView::try_from)
            .transpose()
            .map_err(|_| ProductDetailsReadError::ProductDetailsReadModelInvalid)
    }
}

pub(super) const SELECT_PRODUCT_DETAILS: &str = r#"
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
        p.price_estimate_max_currency, p.state, p.lifecycle, p.url,
        p.product_images, p.auction_start, p.auction_end, p.created, p.updated,
        $2::uuid AS personalization_user_id,
        authenticated_user.prohibited_content_consent AS user_prohibited_content_consent,
        authenticated_user.tier AS user_tier,
        watchlist.notifications AS watchlist_notifications,
        selected_match.user_search_filter_id AS selected_match_user_search_filter_id,
        selected_match.user_search_filter_name AS selected_match_user_search_filter_name,
        selected_match.enhanced_match_reason AS selected_match_reason,
        selected_match.feedback AS selected_match_feedback,
        selected_match.month_position AS selected_match_month_position
    FROM products p
    JOIN shops shop ON shop.shop_id = p.shop_id
    JOIN shops seller ON seller.shop_id = p.seller_id
    LEFT JOIN users authenticated_user ON authenticated_user.user_id = $2
    LEFT JOIN product_watchlist watchlist
        ON watchlist.user_id = $2
        AND watchlist.product_id = p.product_id
    LEFT JOIN LATERAL (
        SELECT
            matched.user_search_filter_id,
            matched.user_search_filter_name,
            matched.enhanced_match_reason,
            matched.feedback,
            CASE
                WHEN authenticated_user.tier = 'FREE' THEN (
                    SELECT COUNT(*)
                    FROM search_filter_matches monthly_match
                    WHERE monthly_match.user_id = $2
                        AND monthly_match.created >= (
                            date_trunc('month', matched.created AT TIME ZONE 'UTC') AT TIME ZONE 'UTC'
                        )
                        AND (
                            monthly_match.created < matched.created
                            OR (
                                monthly_match.created = matched.created
                                AND (
                                    monthly_match.user_search_filter_id < matched.user_search_filter_id
                                    OR (
                                        monthly_match.user_search_filter_id = matched.user_search_filter_id
                                        AND monthly_match.product_id <= matched.product_id
                                    )
                                )
                            )
                        )
                )
                ELSE NULL
            END AS month_position
        FROM search_filter_matches matched
        WHERE matched.user_id = $2
            AND matched.product_id = p.product_id
        ORDER BY matched.created ASC, matched.user_search_filter_id ASC
        LIMIT 1
    ) AS selected_match ON TRUE
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

impl TryFrom<ProductDetailsRow> for PersonalizedProductDetailsView {
    type Error = ();

    fn try_from(row: ProductDetailsRow) -> Result<Self, Self::Error> {
        let address = address(&row)?;
        let parsed_images = images(row.product_images.clone())?;
        let user_state = user_state(&row, &parsed_images)?;
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

        Ok(Personalized {
            item: ProductDetailsView {
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
                },
                price: product_price,
                price_estimate_min: product_price_estimate_min,
                price_estimate_max: product_price_estimate_max,
                currency,
                state: product_state(&row.state)?,
                lifecycle: lifecycle(&row.lifecycle)?,
                view_url: append_utm_params(url.clone()),
                url,
                images: parsed_images,
                auction: ProductAuction {
                    start: row.auction_start,
                    end: row.auction_end,
                },
                created: row.created,
                updated: row.updated,
            },
            user_state,
        })
    }
}

fn user_state(
    row: &ProductDetailsRow,
    images: &IndexSet<ProductImage>,
) -> Result<Option<ProductUserState>, ()> {
    if row.personalization_user_id.is_none() {
        return Ok(None);
    }

    let (stored_consent, tier) = match (
        row.user_prohibited_content_consent,
        row.user_tier.as_deref(),
    ) {
        (Some(consent), Some("FREE")) => (consent, "FREE"),
        (Some(consent), Some("PRO")) => (consent, "PRO"),
        (Some(consent), Some("ULTIMATE")) => (consent, "ULTIMATE"),
        _ => return Err(()),
    };

    let consent = images
        .iter()
        .all(|image| image.prohibited_content.is_safe())
        || stored_consent;
    let search_filter = search_filter_user_state(row, Some(tier))?;

    Ok(Some(ProductUserState {
        watchlist: WatchlistUserState {
            watching: row.watchlist_notifications.is_some(),
            notifications: row.watchlist_notifications.unwrap_or(false),
        },
        prohibited_content: ProhibitedContentUserState { consent },
        notification: Default::default(),
        search_filter,
    }))
}

fn search_filter_user_state(
    row: &ProductDetailsRow,
    tier: Option<&str>,
) -> Result<SearchFilterUserState, ()> {
    let Some(user_search_filter_id) = row.selected_match_user_search_filter_id else {
        if row.selected_match_user_search_filter_name.is_some()
            || row.selected_match_reason.is_some()
            || row.selected_match_feedback.is_some()
            || row.selected_match_month_position.is_some()
        {
            return Err(());
        }
        return Ok(SearchFilterUserState::default());
    };

    let hidden = match tier.ok_or(())? {
        "FREE" => row.selected_match_month_position.ok_or(())? > 10,
        "PRO" | "ULTIMATE" => {
            if row.selected_match_month_position.is_some() {
                return Err(());
            }
            false
        }
        _ => return Err(()),
    };

    Ok(SearchFilterUserState {
        matched: true,
        hidden,
        user_search_filter_id: Some(UserSearchFilterId::from(user_search_filter_id)),
        user_search_filter_name: row
            .selected_match_user_search_filter_name
            .clone()
            .map(UserSearchFilterName::from),
        match_reason: row
            .selected_match_reason
            .clone()
            .map(EnhancedMatchReason::from),
        match_feedback: row.selected_match_feedback,
    })
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

pub(crate) fn images(value: serde_json::Value) -> Result<IndexSet<ProductImage>, ()> {
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
