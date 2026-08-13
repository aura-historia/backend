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
use user_core::sort_user_field::SortUserField;
use user_core::tier::UserTier;
use user_core::user::{RehydratedUserState, User, UserAccount, UserPreferences, UserProfile};
use user_service::ports::{UserDetailsView, UserStorageVersion, VersionedUser};
use user_service::use_cases::queries::find_user_by_stripe_customer_id::UserStripeLookupView;
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

pub(crate) fn sort_user_field_columns(field: SortUserField) -> &'static [&'static str] {
    match field {
        SortUserField::Name => &["first_name", "last_name"],
        SortUserField::Email => &["email"],
        SortUserField::FirstName => &["first_name"],
        SortUserField::LastName => &["last_name"],
        SortUserField::Tier => &["tier"],
        SortUserField::Role => &["role"],
        SortUserField::Created => &["created"],
        SortUserField::Updated => &["updated"],
    }
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

pub(crate) fn parse_optional_language(
    value: Option<&str>,
) -> Result<Option<Language>, UserRowMappingError> {
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

pub(crate) fn parse_optional_currency(
    value: Option<&str>,
) -> Result<Option<Currency>, UserRowMappingError> {
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

pub(crate) fn parse_tier(value: &str) -> Result<UserTier, UserRowMappingError> {
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

#[cfg(test)]
mod tests {
    use super::*;
    use common::version::InvalidVersionError;
    use user_service::use_cases::queries::find_user_by_stripe_customer_id::UserStripeLookupView;

    #[test]
    fn should_bind_domain_values_for_sql() {
        let address = StructuredAddress {
            country: Some(CountryCode::GBR),
            ..Default::default()
        };

        assert_eq!(Some("en"), bind_language(Some(Language::En)));
        assert_eq!(Some("GBP"), bind_currency(Some(Currency::Gbp)));
        assert_eq!(
            Some("METRIC"),
            bind_measurement_unit(Some(MeasurementUnit::Metric))
        );
        assert_eq!(
            Some("IMPERIAL"),
            bind_measurement_unit(Some(MeasurementUnit::Imperial))
        );
        assert_eq!("FREE", bind_tier(UserTier::Free));
        assert_eq!("PRO", bind_tier(UserTier::Pro));
        assert_eq!("ULTIMATE", bind_tier(UserTier::Ultimate));
        assert_eq!("USER", bind_role(UserRole::User));
        assert_eq!("ADMIN", bind_role(UserRole::Admin));
        assert_eq!(Some("GBR".to_owned()), bind_country(Some(&address)));
        assert_eq!(None, bind_country(None));
    }

    #[test]
    fn should_map_countries_for_continents_in_stable_order() {
        let countries =
            countries_for_continents(&std::collections::HashSet::from([Continent::Europe]));

        assert!(countries.windows(2).all(|pair| pair[0] <= pair[1]));
        assert!(countries.contains(&"GBR".to_owned()));
        assert!(!countries.contains(&"USA".to_owned()));
    }

    #[test]
    fn should_map_user_sort_fields_to_postgres_columns() {
        assert_eq!(
            &["first_name", "last_name"],
            sort_user_field_columns(SortUserField::Name)
        );
        assert_eq!(&["email"], sort_user_field_columns(SortUserField::Email));
        assert_eq!(
            &["first_name"],
            sort_user_field_columns(SortUserField::FirstName)
        );
        assert_eq!(
            &["last_name"],
            sort_user_field_columns(SortUserField::LastName)
        );
        assert_eq!(&["tier"], sort_user_field_columns(SortUserField::Tier));
        assert_eq!(&["role"], sort_user_field_columns(SortUserField::Role));
        assert_eq!(
            &["created"],
            sort_user_field_columns(SortUserField::Created)
        );
        assert_eq!(
            &["updated"],
            sort_user_field_columns(SortUserField::Updated)
        );
    }

    #[test]
    fn should_map_full_user_row_to_views() {
        let row = user_row();

        let details = UserDetailsView::try_from(row).unwrap_or_else(|error| {
            panic!("failed to map details: {error}");
        });

        assert_eq!(Email::try_from("ada@example.com").ok(), Some(details.email));
        assert_eq!(Some(Language::En), details.language);
        assert_eq!(Some(Currency::Gbp), details.currency);
        assert_eq!(Some(MeasurementUnit::Imperial), details.measurement_unit);
        assert_eq!(UserTier::Pro, details.tier);
        assert_eq!(UserRole::Admin, details.role);
        assert!(details.structured_address.is_some());
        assert!(details.geo_address.is_some());
    }

    #[test]
    fn should_map_empty_optional_address_and_preferences() {
        let row = UserRow {
            first_name: None,
            last_name: None,
            language: None,
            currency: None,
            measurement_unit: None,
            structured_address_addressline: None,
            structured_address_addressline_extra: None,
            structured_address_locality: None,
            structured_address_region: None,
            structured_address_postal_code: None,
            structured_address_country: None,
            geo_address_lat: None,
            geo_address_lon: None,
            ..user_row()
        };

        let summary = UserSummary::try_from(row).unwrap_or_else(|error| {
            panic!("failed to map summary: {error}");
        });

        assert!(summary.first_name.is_none());
        assert!(summary.last_name.is_none());
    }

    #[test]
    fn should_report_mapping_errors_for_invalid_scalar_values() {
        assert!(matches!(
            UserDetailsView::try_from(UserRow {
                email: "not-an-email".to_owned(),
                ..user_row()
            }),
            Err(UserRowMappingError::InvalidEmail)
        ));
        assert!(matches!(
            VersionedUser::try_from(UserRow {
                language: Some("xx".to_owned()),
                ..user_row()
            }),
            Err(UserRowMappingError::InvalidLanguage)
        ));
        assert!(matches!(
            UserDetailsView::try_from(UserRow {
                currency: Some("BAD".to_owned()),
                ..user_row()
            }),
            Err(UserRowMappingError::InvalidCurrency)
        ));
        assert!(matches!(
            UserDetailsView::try_from(UserRow {
                measurement_unit: Some("BAD".to_owned()),
                ..user_row()
            }),
            Err(UserRowMappingError::InvalidMeasurementUnit)
        ));
        assert!(matches!(
            UserSummary::try_from(UserRow {
                tier: "BAD".to_owned(),
                ..user_row()
            }),
            Err(UserRowMappingError::InvalidTier)
        ));
        assert!(matches!(
            UserSummary::try_from(UserRow {
                role: "BAD".to_owned(),
                ..user_row()
            }),
            Err(UserRowMappingError::InvalidRole)
        ));
    }

    #[test]
    fn should_report_mapping_errors_for_address_stripe_and_version_branches() {
        assert!(matches!(
            UserDetailsView::try_from(UserRow {
                structured_address_country: Some("BAD".to_owned()),
                ..user_row()
            }),
            Err(UserRowMappingError::InvalidCountry)
        ));
        assert!(matches!(
            UserDetailsView::try_from(UserRow {
                geo_address_lon: None,
                ..user_row()
            }),
            Err(UserRowMappingError::IncompleteGeoAddress)
        ));
        assert!(matches!(
            UserStripeLookupView::try_from(UserRow {
                stripe_customer_id: None,
                ..user_row()
            }),
            Err(UserRowMappingError::MissingStripeCustomerId)
        ));
        assert!(matches!(
            VersionedUser::try_from(UserRow {
                version: 0,
                ..user_row()
            }),
            Err(UserRowMappingError::InvalidVersion(
                InvalidVersionError::Zero
            ))
        ));
        assert!(matches!(
            VersionedUser::try_from(UserRow {
                geo_address_lat: Some(91.0),
                ..user_row()
            }),
            Err(UserRowMappingError::InvalidUser(_))
        ));
    }

    fn user_row() -> UserRow {
        UserRow {
            user_id: uuid::Uuid::nil(),
            email: "ada@example.com".to_owned(),
            first_name: Some("Ada".to_owned()),
            last_name: Some("Lovelace".to_owned()),
            language: Some("en".to_owned()),
            currency: Some("GBP".to_owned()),
            measurement_unit: Some("IMPERIAL".to_owned()),
            prohibited_content_consent: true,
            tier: "PRO".to_owned(),
            role: "ADMIN".to_owned(),
            stripe_customer_id: Some("cus_test".to_owned()),
            structured_address_addressline: Some("1 Test Street".to_owned()),
            structured_address_addressline_extra: None,
            structured_address_locality: Some("London".to_owned()),
            structured_address_region: None,
            structured_address_postal_code: Some("SW1A".to_owned()),
            structured_address_country: Some("GBR".to_owned()),
            geo_address_lat: Some(51.5),
            geo_address_lon: Some(-0.1),
            version: 1,
            created: OffsetDateTime::UNIX_EPOCH,
            updated: OffsetDateTime::UNIX_EPOCH,
        }
    }
}
