use common::currency::domain::Currency;
use common::language::domain::Language;
use common::measurement_unit::domain::MeasurementUnit;
use common::stripe_customer_id::StripeCustomerId;
use common::user_id::UserId;
use geo::core::address::{GeoAddress, StructuredAddress};
use geo::core::continent::Continent;
use isocountry::CountryCode;
use serde_email::Email;
use sqlx::FromRow;
use time::OffsetDateTime;
use user_core::first_name::FirstName;
use user_core::last_name::LastName;
use user_core::role::UserRole;
use user_core::tier::UserTier;
use user_core::user::{RehydratedUserState, User, UserAccount, UserPreferences, UserProfile};
use user_service::ports::{UserStorageVersion, VersionedUser};
use user_service::use_cases::queries::find_user_by_stripe_customer_id::UserStripeLookupView;
use user_service::use_cases::queries::get_user::UserDetailsView;
use user_service::use_cases::queries::search_users::UserSummary;

#[allow(dead_code)]
#[derive(Debug, FromRow)]
pub(crate) struct UserRow {
    pub user_id: uuid::Uuid,
    pub email: String,
    pub first_name: Option<String>,
    pub last_name: Option<String>,
    pub language: Option<String>,
    pub currency: Option<String>,
    pub measurement_unit: Option<String>,
    pub prohibited_content_consent: bool,
    pub tier: String,
    pub role: String,
    pub stripe_customer_id: Option<String>,
    pub structured_address_addressline: Option<String>,
    pub structured_address_addressline_extra: Option<String>,
    pub structured_address_locality: Option<String>,
    pub structured_address_region: Option<String>,
    pub structured_address_postal_code: Option<String>,
    pub structured_address_country: Option<String>,
    pub geo_address_lat: Option<f64>,
    pub geo_address_lon: Option<f64>,
    pub version: i64,
    pub created: OffsetDateTime,
    pub updated: OffsetDateTime,
}

pub(crate) fn user_columns() -> &'static str {
    "user_id, email, first_name, last_name, language, currency, measurement_unit, prohibited_content_consent, tier, role, stripe_customer_id, structured_address_addressline, structured_address_addressline_extra, structured_address_locality, structured_address_region, structured_address_postal_code, structured_address_country, geo_address_lat, geo_address_lon, version, created, updated"
}

impl TryFrom<UserRow> for VersionedUser {
    type Error = UserRowMappingError;

    fn try_from(row: UserRow) -> Result<Self, Self::Error> {
        let version = UserStorageVersion::try_from(row.version)?;
        let value = User::rehydrate(RehydratedUserState {
            id: UserId::from(row.user_id),
            email: parse_email(&row.email)?,
            profile: profile_from_row(&row)?,
            preferences: preferences_from_row(&row)?,
            account: account_from_row(&row)?,
        })?;

        Ok(VersionedUser::new(value, version))
    }
}

impl TryFrom<UserRow> for UserDetailsView {
    type Error = UserRowMappingError;

    fn try_from(row: UserRow) -> Result<Self, Self::Error> {
        Ok(Self {
            user_id: UserId::from(row.user_id),
            email: parse_email(&row.email)?,
            first_name: row.first_name.clone().map(FirstName::from),
            last_name: row.last_name.clone().map(LastName::from),
            language: parse_optional_language(row.language.as_deref())?,
            currency: parse_optional_currency(row.currency.as_deref())?,
            measurement_unit: parse_optional_measurement_unit(row.measurement_unit.as_deref())?,
            prohibited_content_consent: row.prohibited_content_consent,
            tier: parse_tier(&row.tier)?,
            role: parse_role(&row.role)?,
            stripe_customer_id: row.stripe_customer_id.clone().map(StripeCustomerId::from),
            structured_address: structured_address_from_row(&row)?,
            geo_address: geo_address_from_row(&row)?,
        })
    }
}

impl TryFrom<UserRow> for UserStripeLookupView {
    type Error = UserRowMappingError;

    fn try_from(row: UserRow) -> Result<Self, Self::Error> {
        Ok(Self {
            user_id: UserId::from(row.user_id),
            email: parse_email(&row.email)?,
            tier: parse_tier(&row.tier)?,
            role: parse_role(&row.role)?,
            stripe_customer_id: row
                .stripe_customer_id
                .ok_or(UserRowMappingError::MissingStripeCustomerId)
                .map(StripeCustomerId::from)?,
        })
    }
}

impl TryFrom<UserRow> for UserSummary {
    type Error = UserRowMappingError;

    fn try_from(row: UserRow) -> Result<Self, Self::Error> {
        Ok(Self {
            user_id: UserId::from(row.user_id),
            email: parse_email(&row.email)?,
            first_name: row.first_name.clone().map(FirstName::from),
            last_name: row.last_name.clone().map(LastName::from),
            tier: parse_tier(&row.tier)?,
            role: parse_role(&row.role)?,
            stripe_customer_id: row.stripe_customer_id.map(StripeCustomerId::from),
        })
    }
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum UserRowMappingError {
    #[error("invalid user email")]
    InvalidEmail,
    #[error("invalid user language")]
    InvalidLanguage,
    #[error("invalid user currency")]
    InvalidCurrency,
    #[error("invalid user measurement unit")]
    InvalidMeasurementUnit,
    #[error("invalid user tier")]
    InvalidTier,
    #[error("invalid user role")]
    InvalidRole,
    #[error("missing user stripe customer id")]
    MissingStripeCustomerId,
    #[error("invalid user country")]
    InvalidCountry,
    #[error("incomplete user geo address")]
    IncompleteGeoAddress,
    #[error("invalid user version")]
    InvalidVersion(#[from] common::version::InvalidVersionError),
    #[error("invalid rehydrated user")]
    InvalidUser(#[from] user_core::user::RehydrateUserError),
}

pub(crate) fn bind_language(value: Option<Language>) -> Option<&'static str> {
    value.map(|language| language.as_str())
}

pub(crate) fn bind_currency(value: Option<Currency>) -> Option<&'static str> {
    value.map(|currency| currency.as_str())
}

pub(crate) fn bind_measurement_unit(value: Option<MeasurementUnit>) -> Option<&'static str> {
    value.map(|unit| match unit {
        MeasurementUnit::Metric => "METRIC",
        MeasurementUnit::Imperial => "IMPERIAL",
    })
}

pub(crate) fn bind_tier(value: UserTier) -> &'static str {
    match value {
        UserTier::Free => "FREE",
        UserTier::Pro => "PRO",
        UserTier::Ultimate => "ULTIMATE",
    }
}

pub(crate) fn bind_role(value: UserRole) -> &'static str {
    match value {
        UserRole::User => "USER",
        UserRole::Admin => "ADMIN",
    }
}

pub(crate) fn bind_country(address: Option<&StructuredAddress>) -> Option<String> {
    address
        .and_then(|value| value.country)
        .map(|country| country.alpha3().to_owned())
}

pub(crate) fn version_to_i64(version: UserStorageVersion) -> i64 {
    i64::try_from(version.into_inner()).map_or(i64::MAX, |value| value)
}

pub(crate) fn countries_for_continents(
    continents: &std::collections::HashSet<Continent>,
) -> Vec<String> {
    let mut countries = CountryCode::iter()
        .copied()
        .filter(|country| continents.contains(&Continent::from(*country)))
        .map(|country| country.alpha3().to_owned())
        .collect::<Vec<_>>();
    countries.sort();
    countries
}

fn profile_from_row(row: &UserRow) -> Result<UserProfile, UserRowMappingError> {
    Ok(UserProfile {
        first_name: row.first_name.clone().map(FirstName::from),
        last_name: row.last_name.clone().map(LastName::from),
        structured_address: structured_address_from_row(row)?,
        geo_address: geo_address_from_row(row)?,
    })
}

fn preferences_from_row(row: &UserRow) -> Result<UserPreferences, UserRowMappingError> {
    Ok(UserPreferences {
        language: parse_optional_language(row.language.as_deref())?,
        currency: parse_optional_currency(row.currency.as_deref())?,
        measurement_unit: parse_optional_measurement_unit(row.measurement_unit.as_deref())?,
        prohibited_content_consent: row.prohibited_content_consent,
    })
}

fn account_from_row(row: &UserRow) -> Result<UserAccount, UserRowMappingError> {
    Ok(UserAccount {
        tier: parse_tier(&row.tier)?,
        role: parse_role(&row.role)?,
        stripe_customer_id: row.stripe_customer_id.clone().map(StripeCustomerId::from),
    })
}

fn structured_address_from_row(
    row: &UserRow,
) -> Result<Option<StructuredAddress>, UserRowMappingError> {
    let country = parse_country(row.structured_address_country.as_deref())?;
    let address = StructuredAddress {
        addressline: row.structured_address_addressline.clone(),
        addressline_extra: row.structured_address_addressline_extra.clone(),
        locality: row.structured_address_locality.clone(),
        region: row.structured_address_region.clone(),
        postal_code: row.structured_address_postal_code.clone(),
        country,
        continent: country.map(Continent::from),
    };

    Ok((!address.is_empty()).then_some(address))
}

fn geo_address_from_row(row: &UserRow) -> Result<Option<GeoAddress>, UserRowMappingError> {
    match (row.geo_address_lat, row.geo_address_lon) {
        (Some(lat), Some(lon)) => Ok(Some(GeoAddress { lat, lon })),
        (None, None) => Ok(None),
        _ => Err(UserRowMappingError::IncompleteGeoAddress),
    }
}

fn parse_email(value: &str) -> Result<Email, UserRowMappingError> {
    Email::try_from(value).map_err(|_| UserRowMappingError::InvalidEmail)
}

fn parse_country(value: Option<&str>) -> Result<Option<CountryCode>, UserRowMappingError> {
    value
        .map(|value| {
            CountryCode::for_alpha3(value).map_err(|_| UserRowMappingError::InvalidCountry)
        })
        .transpose()
}

fn parse_optional_language(value: Option<&str>) -> Result<Option<Language>, UserRowMappingError> {
    value.map(parse_language).transpose()
}

fn parse_language(value: &str) -> Result<Language, UserRowMappingError> {
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
        _ => Err(UserRowMappingError::InvalidLanguage),
    }
}

fn parse_optional_currency(value: Option<&str>) -> Result<Option<Currency>, UserRowMappingError> {
    value.map(parse_currency).transpose()
}

fn parse_currency(value: &str) -> Result<Currency, UserRowMappingError> {
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
        _ => Err(UserRowMappingError::InvalidCurrency),
    }
}

fn parse_optional_measurement_unit(
    value: Option<&str>,
) -> Result<Option<MeasurementUnit>, UserRowMappingError> {
    value.map(parse_measurement_unit).transpose()
}

fn parse_measurement_unit(value: &str) -> Result<MeasurementUnit, UserRowMappingError> {
    match value.to_ascii_uppercase().as_str() {
        "METRIC" => Ok(MeasurementUnit::Metric),
        "IMPERIAL" => Ok(MeasurementUnit::Imperial),
        _ => Err(UserRowMappingError::InvalidMeasurementUnit),
    }
}

fn parse_tier(value: &str) -> Result<UserTier, UserRowMappingError> {
    match value.to_ascii_uppercase().as_str() {
        "FREE" => Ok(UserTier::Free),
        "PRO" => Ok(UserTier::Pro),
        "ULTIMATE" => Ok(UserTier::Ultimate),
        _ => Err(UserRowMappingError::InvalidTier),
    }
}

fn parse_role(value: &str) -> Result<UserRole, UserRowMappingError> {
    match value.to_ascii_uppercase().as_str() {
        "USER" => Ok(UserRole::User),
        "ADMIN" => Ok(UserRole::Admin),
        _ => Err(UserRowMappingError::InvalidRole),
    }
}
