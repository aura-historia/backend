use geo::core::address::{GeoAddress, StructuredAddress};
use geo::core::continent::Continent;
use isocountry::CountryCode;
use localization::Language;
use money::Currency;
use serde::{Deserialize, Serialize};
use serde_email::Email;
use shop_core::affiliate_configuration::AffiliateConfiguration;
use shop_core::domain::Domain;
use shop_core::lifecycle::ShopLifecycle;
use shop_core::partner_status::ShopPartnerStatus;
use shop_core::shop::{
    RehydratedShopState, Shop, ShopAddress, ShopContact, ShopPresentation, ShopifyIntegration,
    WoocommerceIntegration,
};
use shop_core::shop_id::ShopId;
use shop_core::shop_name::ShopName;
use shop_core::shop_slug_id::ShopSlugId;
use shop_core::shop_type::ShopType;
use shop_core::woocommerce_webhook_secret::WoocommerceWebhookSecret;
use shop_service::ports::{ShopStorageVersion, StoredShop};
use shop_service::use_cases::queries::get_shop::ShopDetailsView;
use shop_service::use_cases::queries::search_shops::ShopSummary;
use std::collections::HashSet;
use strum::IntoEnumIterator;
use time::OffsetDateTime;
use url::Url;

#[derive(Debug, Clone, sqlx::FromRow)]
pub(crate) struct ShopRow {
    pub shop_id: uuid::Uuid,
    pub shop_slug_id: String,
    pub name: String,
    pub shop_type: String,
    pub partner_status: String,
    pub lifecycle: String,
    pub shop_domains: Vec<String>,
    pub shopify_domain: Option<String>,
    pub shopify_currency: Option<String>,
    pub shopify_language: Option<String>,
    pub woocommerce_webhook_secret: Option<String>,
    pub woocommerce_currency: Option<String>,
    pub woocommerce_language: Option<String>,
    pub url: Option<String>,
    pub image: Option<String>,
    pub structured_address_addressline: Option<String>,
    pub structured_address_addressline_extra: Option<String>,
    pub structured_address_locality: Option<String>,
    pub structured_address_region: Option<String>,
    pub structured_address_postal_code: Option<String>,
    pub structured_address_country: Option<String>,
    pub geo_address_lat: Option<f64>,
    pub geo_address_lon: Option<f64>,
    pub phone: Option<String>,
    pub email: Option<String>,
    pub affiliate_configuration: Option<serde_json::Value>,
    pub version: i64,
    pub created: OffsetDateTime,
    pub updated: OffsetDateTime,
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub(crate) struct ShopSummaryRow {
    pub shop_id: uuid::Uuid,
    pub shop_slug_id: String,
    pub name: String,
    pub shop_type: String,
    pub partner_status: String,
    pub shop_domains: Vec<String>,
    pub image: Option<String>,
    pub created: OffsetDateTime,
    pub updated: OffsetDateTime,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub(crate) enum ShopRowMappingError {
    #[error("invalid shop slug persisted")]
    InvalidSlug,
    #[error("invalid shop domain persisted")]
    InvalidDomain,
    #[error("invalid shop type persisted")]
    InvalidShopType,
    #[error("invalid shop partner status persisted")]
    InvalidPartnerStatus,
    #[error("invalid shop lifecycle persisted")]
    InvalidLifecycle,
    #[error("invalid shopify currency persisted")]
    InvalidShopifyCurrency,
    #[error("invalid shopify language persisted")]
    InvalidShopifyLanguage,
    #[error("incomplete shopify integration persisted")]
    IncompleteShopifyIntegration,
    #[error("invalid woocommerce currency persisted")]
    InvalidWoocommerceCurrency,
    #[error("invalid woocommerce language persisted")]
    InvalidWoocommerceLanguage,
    #[error("invalid shop url persisted")]
    InvalidUrl,
    #[error("invalid shop image url persisted")]
    InvalidImageUrl,
    #[error("invalid shop country persisted")]
    InvalidCountry,
    #[error("incomplete geo address persisted")]
    IncompleteGeoAddress,
    #[error("invalid shop email persisted")]
    InvalidEmail,
    #[error("invalid affiliate configuration persisted")]
    InvalidAffiliateConfiguration,
    #[error("invalid shop version persisted")]
    InvalidVersion,
    #[error("invalid shop aggregate state persisted")]
    InvalidAggregateState,
}

const SHOP_COLUMNS: &str = r#"
    shop_id, shop_slug_id, name, shop_type, partner_status, lifecycle, shop_domains,
    shopify_domain, shopify_currency, shopify_language,
    woocommerce_webhook_secret, woocommerce_currency, woocommerce_language,
    url, image,
    structured_address_addressline, structured_address_addressline_extra,
    structured_address_locality, structured_address_region, structured_address_postal_code,
    structured_address_country, geo_address_lat, geo_address_lon,
    phone, email, affiliate_configuration, version, created, updated
"#;

const SHOP_SUMMARY_COLUMNS: &str = r#"
    shop_id, shop_slug_id, name, shop_type, partner_status, shop_domains, image, created, updated
"#;

pub(crate) fn shop_columns() -> &'static str {
    SHOP_COLUMNS
}

pub(crate) fn shop_summary_columns() -> &'static str {
    SHOP_SUMMARY_COLUMNS
}

impl TryFrom<ShopRow> for StoredShop {
    type Error = ShopRowMappingError;

    fn try_from(row: ShopRow) -> Result<Self, Self::Error> {
        let version = ShopStorageVersion::try_from(row.version)
            .map_err(|_| ShopRowMappingError::InvalidVersion)?;
        Ok(Self {
            shop: row.to_shop()?,
            version,
            created: row.created,
            updated: row.updated,
        })
    }
}

impl TryFrom<ShopRow> for ShopDetailsView {
    type Error = ShopRowMappingError;

    fn try_from(row: ShopRow) -> Result<Self, Self::Error> {
        let url = parse_optional_url(row.url.as_deref(), ShopRowMappingError::InvalidUrl)?;
        let affiliate_configuration = parse_affiliate_configuration(&row.affiliate_configuration)?;
        let view_url = derive_view_url(url.as_ref(), affiliate_configuration.as_ref());
        let address = structured_address_from_row(&row)?;
        let contact = contact_from_row(&row)?;
        let shopify = shopify_from_row(&row)?;
        let woocommerce = woocommerce_from_row(&row)?;

        Ok(Self {
            shop_id: ShopId::from(row.shop_id),
            shop_slug_id: ShopSlugId::raw(&row.shop_slug_id)
                .map_err(|_| ShopRowMappingError::InvalidSlug)?,
            name: ShopName::from(row.name),
            shop_type: parse_shop_type(&row.shop_type)?,
            domains: parse_domains(&row.shop_domains)?,
            shopify_domain: shopify.as_ref().map(|value| value.domain.clone()),
            shopify_currency: shopify.as_ref().and_then(|value| value.currency),
            shopify_language: shopify.as_ref().and_then(|value| value.language),
            woocommerce_currency: woocommerce.as_ref().and_then(|value| value.currency),
            woocommerce_language: woocommerce.as_ref().and_then(|value| value.language),
            url,
            view_url,
            image: parse_optional_url(row.image.as_deref(), ShopRowMappingError::InvalidImageUrl)?,
            structured_address: address.as_ref().map(|value| value.structured.clone()),
            geo_address: address.and_then(|value| value.geo),
            phone: contact.phone,
            email: contact.email,
            partner_status: parse_partner_status(&row.partner_status)?,
            affiliate_configuration,
            created: row.created,
            updated: row.updated,
        })
    }
}

impl TryFrom<ShopSummaryRow> for ShopSummary {
    type Error = ShopRowMappingError;

    fn try_from(row: ShopSummaryRow) -> Result<Self, Self::Error> {
        let mut domains = parse_domains(&row.shop_domains)?
            .into_iter()
            .collect::<Vec<_>>();
        domains.sort_by(|a, b| a.as_str().cmp(b.as_str()));

        Ok(Self {
            shop_id: ShopId::from(row.shop_id),
            shop_slug_id: ShopSlugId::raw(&row.shop_slug_id)
                .map_err(|_| ShopRowMappingError::InvalidSlug)?,
            name: ShopName::from(row.name),
            shop_type: parse_shop_type(&row.shop_type)?,
            partner_status: parse_partner_status(&row.partner_status)?,
            domains,
            image: parse_optional_url(row.image.as_deref(), ShopRowMappingError::InvalidImageUrl)?,
            created: row.created,
            updated: row.updated,
        })
    }
}

impl ShopRow {
    fn to_shop(&self) -> Result<Shop, ShopRowMappingError> {
        let address = structured_address_from_row(self)?;
        let contact = contact_from_row(self)?;
        let state = RehydratedShopState {
            id: ShopId::from(self.shop_id),
            slug_id: ShopSlugId::raw(&self.shop_slug_id)
                .map_err(|_| ShopRowMappingError::InvalidSlug)?,
            name: ShopName::from(self.name.clone()),
            shop_type: parse_shop_type(&self.shop_type)?,
            domains: parse_domains(&self.shop_domains)?,
            shopify: shopify_from_row(self)?,
            woocommerce: woocommerce_from_row(self)?,
            presentation: ShopPresentation {
                url: parse_optional_url(self.url.as_deref(), ShopRowMappingError::InvalidUrl)?,
                image: parse_optional_url(
                    self.image.as_deref(),
                    ShopRowMappingError::InvalidImageUrl,
                )?,
            },
            address,
            contact,
            partner_status: parse_partner_status(&self.partner_status)?,
            lifecycle: parse_lifecycle(&self.lifecycle)?,
            affiliate_configuration: parse_affiliate_configuration(&self.affiliate_configuration)?,
        };

        Shop::rehydrate(state).map_err(|_| ShopRowMappingError::InvalidAggregateState)
    }
}

pub(crate) fn bind_domains(shop: &Shop) -> Vec<String> {
    let mut domains = shop
        .domains()
        .iter()
        .map(|domain| domain.as_str().to_owned())
        .collect::<Vec<_>>();
    domains.sort();
    domains
}

pub(crate) fn bind_country(address: Option<&ShopAddress>) -> Option<String> {
    address
        .and_then(|value| value.structured.country)
        .map(|country| country.alpha3().to_owned())
}

pub(crate) fn bind_affiliate_configuration(
    affiliate_configuration: Option<&AffiliateConfiguration>,
) -> Option<serde_json::Value> {
    affiliate_configuration.map(|configuration| match configuration {
        AffiliateConfiguration::Partnerize { camref } => serde_json::json!({
            "type": "PARTNERIZE",
            "camref": camref,
        }),
    })
}

pub(crate) fn bind_shop_type(value: ShopType) -> &'static str {
    value.as_str()
}

pub(crate) fn bind_partner_status(value: ShopPartnerStatus) -> &'static str {
    value.as_str()
}

pub(crate) fn bind_lifecycle(value: ShopLifecycle) -> &'static str {
    value.as_str()
}

pub(crate) fn bind_currency(value: Option<Currency>) -> Option<&'static str> {
    value.map(|currency| currency.as_str())
}

pub(crate) fn bind_language(value: Option<Language>) -> Option<&'static str> {
    value.map(|language| language.as_str())
}

pub(crate) fn version_to_i64(version: ShopStorageVersion) -> i64 {
    i64::try_from(version.into_inner()).unwrap_or(i64::MAX)
}

pub(crate) fn countries_for_continents(continents: &HashSet<Continent>) -> Vec<String> {
    let mut countries = CountryCode::iter()
        .copied()
        .filter(|country| continents.contains(&Continent::from(*country)))
        .map(|country| country.alpha3().to_owned())
        .collect::<Vec<_>>();
    countries.sort();
    countries
}

fn parse_domains(values: &[String]) -> Result<HashSet<Domain>, ShopRowMappingError> {
    values
        .iter()
        .map(|value| {
            Domain::try_from(value.as_str()).map_err(|_| ShopRowMappingError::InvalidDomain)
        })
        .collect()
}

fn parse_optional_url(
    value: Option<&str>,
    error: ShopRowMappingError,
) -> Result<Option<Url>, ShopRowMappingError> {
    value
        .map(|value| Url::parse(value).map_err(|_| error))
        .transpose()
}

fn shopify_from_row(row: &ShopRow) -> Result<Option<ShopifyIntegration>, ShopRowMappingError> {
    match row.shopify_domain.as_deref() {
        Some(domain) => Ok(Some(ShopifyIntegration {
            domain: Domain::try_from(domain).map_err(|_| ShopRowMappingError::InvalidDomain)?,
            currency: parse_optional_currency(
                row.shopify_currency.as_deref(),
                ShopRowMappingError::InvalidShopifyCurrency,
            )?,
            language: parse_optional_language(
                row.shopify_language.as_deref(),
                ShopRowMappingError::InvalidShopifyLanguage,
            )?,
        })),
        None if row.shopify_currency.is_some() || row.shopify_language.is_some() => {
            Err(ShopRowMappingError::IncompleteShopifyIntegration)
        }
        None => Ok(None),
    }
}

fn woocommerce_from_row(
    row: &ShopRow,
) -> Result<Option<WoocommerceIntegration>, ShopRowMappingError> {
    let webhook_secret = row
        .woocommerce_webhook_secret
        .as_ref()
        .map(|value| WoocommerceWebhookSecret::from(value.clone()));
    let currency = parse_optional_currency(
        row.woocommerce_currency.as_deref(),
        ShopRowMappingError::InvalidWoocommerceCurrency,
    )?;
    let language = parse_optional_language(
        row.woocommerce_language.as_deref(),
        ShopRowMappingError::InvalidWoocommerceLanguage,
    )?;

    if webhook_secret.is_none() && currency.is_none() && language.is_none() {
        Ok(None)
    } else {
        Ok(Some(WoocommerceIntegration {
            webhook_secret,
            currency,
            language,
        }))
    }
}

fn structured_address_from_row(row: &ShopRow) -> Result<Option<ShopAddress>, ShopRowMappingError> {
    let country = parse_country(row.structured_address_country.as_deref())?;
    let structured = StructuredAddress {
        addressline: row.structured_address_addressline.clone(),
        addressline_extra: row.structured_address_addressline_extra.clone(),
        locality: row.structured_address_locality.clone(),
        region: row.structured_address_region.clone(),
        postal_code: row.structured_address_postal_code.clone(),
        country,
        continent: country.map(Continent::from),
    };
    let geo = geo_address_from_row(row)?;

    if structured.is_empty() && geo.is_none() {
        Ok(None)
    } else {
        Ok(Some(ShopAddress { structured, geo }))
    }
}

fn geo_address_from_row(row: &ShopRow) -> Result<Option<GeoAddress>, ShopRowMappingError> {
    match (row.geo_address_lat, row.geo_address_lon) {
        (Some(lat), Some(lon)) => Ok(Some(GeoAddress { lat, lon })),
        (None, None) => Ok(None),
        _ => Err(ShopRowMappingError::IncompleteGeoAddress),
    }
}

fn contact_from_row(row: &ShopRow) -> Result<ShopContact, ShopRowMappingError> {
    Ok(ShopContact {
        phone: row.phone.clone(),
        email: row
            .email
            .as_deref()
            .map(Email::try_from)
            .transpose()
            .map_err(|_| ShopRowMappingError::InvalidEmail)?,
    })
}

fn append_utm_params(mut url: Url) -> Url {
    if url.query_pairs().any(|(key, _)| key == "utm_source") {
        return url;
    }

    url.query_pairs_mut()
        .append_pair("utm_source", "aura_historia")
        .append_pair("utm_medium", "referral");
    url
}

fn derive_view_url(
    url: Option<&Url>,
    affiliate_configuration: Option<&AffiliateConfiguration>,
) -> Option<Url> {
    url.map(|url| {
        affiliate_configuration
            .map(|configuration| configuration.build_url(url))
            .unwrap_or_else(|| append_utm_params(url.clone()))
    })
}

fn parse_affiliate_configuration(
    value: &Option<serde_json::Value>,
) -> Result<Option<AffiliateConfiguration>, ShopRowMappingError> {
    value
        .clone()
        .map(|value| serde_json::from_value::<AffiliateConfigurationJson>(value).map(Into::into))
        .transpose()
        .map_err(|_| ShopRowMappingError::InvalidAffiliateConfiguration)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "SCREAMING_SNAKE_CASE")]
enum AffiliateConfigurationJson {
    Partnerize { camref: String },
}

impl From<AffiliateConfigurationJson> for AffiliateConfiguration {
    fn from(value: AffiliateConfigurationJson) -> Self {
        match value {
            AffiliateConfigurationJson::Partnerize { camref } => {
                AffiliateConfiguration::Partnerize { camref }
            }
        }
    }
}

fn parse_country(value: Option<&str>) -> Result<Option<CountryCode>, ShopRowMappingError> {
    value
        .map(|value| {
            CountryCode::for_alpha3(value).map_err(|_| ShopRowMappingError::InvalidCountry)
        })
        .transpose()
}

fn parse_optional_currency(
    value: Option<&str>,
    error: ShopRowMappingError,
) -> Result<Option<Currency>, ShopRowMappingError> {
    value.map(|value| parse_currency(value, error)).transpose()
}

fn parse_currency(
    value: &str,
    error: ShopRowMappingError,
) -> Result<Currency, ShopRowMappingError> {
    Currency::iter()
        .find(|currency| currency.as_str() == value)
        .ok_or(error)
}

fn parse_optional_language(
    value: Option<&str>,
    error: ShopRowMappingError,
) -> Result<Option<Language>, ShopRowMappingError> {
    value.map(|value| parse_language(value, error)).transpose()
}

fn parse_language(
    value: &str,
    error: ShopRowMappingError,
) -> Result<Language, ShopRowMappingError> {
    Language::iter()
        .find(|language| language.as_str() == value)
        .ok_or(error)
}

fn parse_shop_type(value: &str) -> Result<ShopType, ShopRowMappingError> {
    ShopType::iter()
        .find(|shop_type| shop_type.as_str() == value)
        .ok_or(ShopRowMappingError::InvalidShopType)
}

fn parse_partner_status(value: &str) -> Result<ShopPartnerStatus, ShopRowMappingError> {
    ShopPartnerStatus::iter()
        .find(|status| status.as_str() == value)
        .ok_or(ShopRowMappingError::InvalidPartnerStatus)
}

fn parse_lifecycle(value: &str) -> Result<ShopLifecycle, ShopRowMappingError> {
    ShopLifecycle::iter()
        .find(|lifecycle| lifecycle.as_str() == value)
        .ok_or(ShopRowMappingError::InvalidLifecycle)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_map_valid_row_to_stored_shop() {
        let row = full_row("Antik Markt", "antik-markt");

        let result = StoredShop::try_from(row);

        assert!(matches!(result, Ok(ref loaded)
            if loaded.shop.name().as_ref() == "Antik Markt"
                && loaded.version == ShopStorageVersion::INITIAL
                && loaded.shop.view_url().as_ref().map(Url::as_str) == Some("https://prf.hn/click/camref:1110lF73C/pubref:aurahistoria/destination:https%3A%2F%2Fexample.com%2Fshop")
        ));
    }

    #[test]
    fn should_reject_row_when_slug_does_not_match_name() {
        let row = full_row("Antik Markt", "wrong");

        let result = StoredShop::try_from(row);

        assert!(matches!(
            result,
            Err(ShopRowMappingError::InvalidAggregateState)
        ));
    }

    #[test]
    fn should_reject_row_when_view_dependency_is_invalid() {
        let mut row = full_row("Antik Markt", "antik-markt");
        row.affiliate_configuration = Some(serde_json::json!({ "type": "UNKNOWN" }));

        let result = ShopDetailsView::try_from(row);

        assert!(matches!(
            result,
            Err(ShopRowMappingError::InvalidAffiliateConfiguration)
        ));
    }

    #[test]
    fn should_derive_utm_view_url_when_no_affiliate_config_exists() {
        let mut row = full_row("Antik Markt", "antik-markt");
        row.affiliate_configuration = None;

        let result = ShopDetailsView::try_from(row);

        assert!(matches!(result, Ok(ref view)
            if view.view_url.as_ref().map(Url::as_str) == Some("https://example.com/shop?utm_source=aura_historia&utm_medium=referral")
        ));
    }

    #[test]
    fn should_map_summary_row_with_sorted_domains() {
        let row = ShopSummaryRow {
            shop_id: uuid::Uuid::new_v4(),
            shop_slug_id: "antik-markt".to_owned(),
            name: "Antik Markt".to_owned(),
            shop_type: "MARKETPLACE".to_owned(),
            partner_status: "PARTNERED".to_owned(),
            shop_domains: vec!["z.example".to_owned(), "a.example".to_owned()],
            image: Some("https://example.com/image.jpg".to_owned()),
            created: OffsetDateTime::UNIX_EPOCH,
            updated: OffsetDateTime::UNIX_EPOCH,
        };

        let result = ShopSummary::try_from(row);

        assert!(matches!(result, Ok(ref summary)
            if summary.domains.iter().map(Domain::as_str).collect::<Vec<_>>() == vec!["a.example", "z.example"]
        ));
    }

    #[test]
    fn should_bind_affiliate_config_without_persisting_view_url() {
        let json = bind_affiliate_configuration(Some(&AffiliateConfiguration::Partnerize {
            camref: "1110lF73C".to_owned(),
        }));

        assert_eq!(
            Some(serde_json::json!({ "type": "PARTNERIZE", "camref": "1110lF73C" })),
            json
        );
    }

    #[test]
    fn should_parse_and_bind_all_canonical_shop_enum_values() {
        for shop_type in ShopType::iter() {
            assert_eq!(Ok(shop_type), parse_shop_type(shop_type.as_str()));
            assert_eq!(shop_type.as_str(), bind_shop_type(shop_type));
        }
        for status in ShopPartnerStatus::iter() {
            assert_eq!(Ok(status), parse_partner_status(status.as_str()));
            assert_eq!(status.as_str(), bind_partner_status(status));
        }
        for lifecycle in ShopLifecycle::iter() {
            assert_eq!(Ok(lifecycle), parse_lifecycle(lifecycle.as_str()));
            assert_eq!(lifecycle.as_str(), bind_lifecycle(lifecycle));
        }

        assert_eq!(
            Err(ShopRowMappingError::InvalidShopType),
            parse_shop_type("UNKNOWN")
        );
        assert_eq!(
            Err(ShopRowMappingError::InvalidPartnerStatus),
            parse_partner_status("UNKNOWN")
        );
        assert_eq!(
            Err(ShopRowMappingError::InvalidLifecycle),
            parse_lifecycle("UNKNOWN")
        );
    }

    #[rstest::rstest]
    #[case("EUR", Ok(Currency::Eur))]
    #[case("GBP", Ok(Currency::Gbp))]
    #[case("USD", Ok(Currency::Usd))]
    #[case("AUD", Ok(Currency::Aud))]
    #[case("CAD", Ok(Currency::Cad))]
    #[case("NZD", Ok(Currency::Nzd))]
    #[case("CNY", Ok(Currency::Cny))]
    #[case("BRL", Ok(Currency::Brl))]
    #[case("PLN", Ok(Currency::Pln))]
    #[case("TRY", Ok(Currency::Try))]
    #[case("JPY", Ok(Currency::Jpy))]
    #[case("CZK", Ok(Currency::Czk))]
    #[case("RUB", Ok(Currency::Rub))]
    #[case("AED", Ok(Currency::Aed))]
    #[case("SAR", Ok(Currency::Sar))]
    #[case("HKD", Ok(Currency::Hkd))]
    #[case("SGD", Ok(Currency::Sgd))]
    #[case("CHF", Ok(Currency::Chf))]
    #[case("XXX", Err(ShopRowMappingError::InvalidShopifyCurrency))]
    #[case("eur", Err(ShopRowMappingError::InvalidShopifyCurrency))]
    fn should_parse_currency_branches(
        #[case] value: &str,
        #[case] expected: Result<Currency, ShopRowMappingError>,
    ) {
        assert_eq!(
            expected,
            parse_currency(value, ShopRowMappingError::InvalidShopifyCurrency)
        );
    }

    #[rstest::rstest]
    #[case("de", Ok(Language::De))]
    #[case("en", Ok(Language::En))]
    #[case("fr", Ok(Language::Fr))]
    #[case("es", Ok(Language::Es))]
    #[case("it", Ok(Language::It))]
    #[case("zh", Ok(Language::Zh))]
    #[case("pt", Ok(Language::Pt))]
    #[case("pl", Ok(Language::Pl))]
    #[case("tr", Ok(Language::Tr))]
    #[case("nl", Ok(Language::Nl))]
    #[case("cs", Ok(Language::Cs))]
    #[case("ja", Ok(Language::Ja))]
    #[case("ru", Ok(Language::Ru))]
    #[case("ar", Ok(Language::Ar))]
    #[case("xx", Err(ShopRowMappingError::InvalidShopifyLanguage))]
    #[case("EN", Err(ShopRowMappingError::InvalidShopifyLanguage))]
    fn should_parse_language_branches(
        #[case] value: &str,
        #[case] expected: Result<Language, ShopRowMappingError>,
    ) {
        assert_eq!(
            expected,
            parse_language(value, ShopRowMappingError::InvalidShopifyLanguage)
        );
    }

    #[rstest::rstest]
    #[case::bad_domain(|row: &mut ShopRow| row.shop_domains = vec!["bad domain".to_owned()], ShopRowMappingError::InvalidDomain)]
    #[case::bad_url(|row: &mut ShopRow| row.url = Some("not-a-url".to_owned()), ShopRowMappingError::InvalidUrl)]
    #[case::bad_image(|row: &mut ShopRow| row.image = Some("not-a-url".to_owned()), ShopRowMappingError::InvalidImageUrl)]
    #[case::bad_country(|row: &mut ShopRow| row.structured_address_country = Some("BAD".to_owned()), ShopRowMappingError::InvalidCountry)]
    #[case::bad_geo(|row: &mut ShopRow| row.geo_address_lon = None, ShopRowMappingError::IncompleteGeoAddress)]
    #[case::bad_email(|row: &mut ShopRow| row.email = Some("not-email".to_owned()), ShopRowMappingError::InvalidEmail)]
    #[case::bad_version(|row: &mut ShopRow| row.version = -1, ShopRowMappingError::InvalidVersion)]
    #[case::bad_shopify_domain(|row: &mut ShopRow| row.shopify_domain = Some("bad domain".to_owned()), ShopRowMappingError::InvalidDomain)]
    #[case::bad_shopify_currency(|row: &mut ShopRow| row.shopify_currency = Some("XXX".to_owned()), ShopRowMappingError::InvalidShopifyCurrency)]
    #[case::bad_shopify_language(|row: &mut ShopRow| row.shopify_language = Some("xx".to_owned()), ShopRowMappingError::InvalidShopifyLanguage)]
    #[case::incomplete_shopify(|row: &mut ShopRow| { row.shopify_domain = None; row.shopify_currency = Some("EUR".to_owned()); }, ShopRowMappingError::IncompleteShopifyIntegration)]
    #[case::bad_woocommerce_currency(|row: &mut ShopRow| row.woocommerce_currency = Some("XXX".to_owned()), ShopRowMappingError::InvalidWoocommerceCurrency)]
    #[case::bad_woocommerce_language(|row: &mut ShopRow| row.woocommerce_language = Some("xx".to_owned()), ShopRowMappingError::InvalidWoocommerceLanguage)]
    fn should_reject_invalid_row_private_mapping_branches(
        #[case] mutate: fn(&mut ShopRow),
        #[case] expected: ShopRowMappingError,
    ) {
        let mut row = full_row("Antik Markt", "antik-markt");
        mutate(&mut row);

        let result = StoredShop::try_from(row);

        assert!(matches!(result, Err(error) if error == expected));
    }

    #[test]
    fn should_map_empty_optional_row_sections_to_none() {
        let mut row = full_row("Antik Markt", "antik-markt");
        row.shopify_domain = None;
        row.shopify_currency = None;
        row.shopify_language = None;
        row.woocommerce_webhook_secret = None;
        row.woocommerce_currency = None;
        row.woocommerce_language = None;
        row.structured_address_addressline = None;
        row.structured_address_addressline_extra = None;
        row.structured_address_locality = None;
        row.structured_address_region = None;
        row.structured_address_postal_code = None;
        row.structured_address_country = None;
        row.geo_address_lat = None;
        row.geo_address_lon = None;
        row.email = None;
        row.phone = None;

        let result = StoredShop::try_from(row);

        assert!(matches!(result, Ok(ref loaded)
            if loaded.shop.address().is_none()
                && loaded.shop.woocommerce().is_none()
                && loaded.shop.address().is_none()
                && loaded.shop.contact() == &ShopContact::default()
        ));
    }

    fn full_row(name: &str, slug: &str) -> ShopRow {
        ShopRow {
            shop_id: uuid::Uuid::new_v4(),
            shop_slug_id: slug.to_owned(),
            name: name.to_owned(),
            shop_type: "COMMERCIAL_DEALER".to_owned(),
            partner_status: "SCRAPED".to_owned(),
            lifecycle: "DRAFTED".to_owned(),
            shop_domains: vec!["example.com".to_owned()],
            shopify_domain: Some("shopify.example".to_owned()),
            shopify_currency: Some("EUR".to_owned()),
            shopify_language: Some("de".to_owned()),
            woocommerce_webhook_secret: Some("secret".to_owned()),
            woocommerce_currency: Some("USD".to_owned()),
            woocommerce_language: Some("en".to_owned()),
            url: Some("https://example.com/shop".to_owned()),
            image: Some("https://example.com/image.jpg".to_owned()),
            structured_address_addressline: Some("Main 1".to_owned()),
            structured_address_addressline_extra: Some("Back".to_owned()),
            structured_address_locality: Some("Berlin".to_owned()),
            structured_address_region: Some("BE".to_owned()),
            structured_address_postal_code: Some("10115".to_owned()),
            structured_address_country: Some("DEU".to_owned()),
            geo_address_lat: Some(52.5),
            geo_address_lon: Some(13.4),
            phone: Some("+49".to_owned()),
            email: Some("mail@example.com".to_owned()),
            affiliate_configuration: Some(serde_json::json!({
                "type": "PARTNERIZE",
                "camref": "1110lF73C"
            })),
            version: 1,
            created: OffsetDateTime::UNIX_EPOCH,
            updated: OffsetDateTime::UNIX_EPOCH,
        }
    }
}
