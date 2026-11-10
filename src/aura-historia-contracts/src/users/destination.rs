use geo::{PostalCode, country_from_code, reference::ReferenceRelease};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use user_core::user_address::{BrowsingDestination, UserAddressId};
use user_service::address_book::DestinationView;

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("invalid browsing destination")]
pub struct InvalidDestinationData;

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "SCREAMING_SNAKE_CASE", deny_unknown_fields)]
#[schemars(transform = crate::wire::schema::describe_kind)]
pub enum DestinationData {
    // A struct variant so `deny_unknown_fields` applies; serde lets internally tagged unit
    // variants ignore extra members.
    /// No browsing destination.
    None {},
    /// One of the user's saved addresses.
    Address {
        /// ID of the saved address (`uaddr_…`).
        #[serde(rename = "addressId")]
        #[schemars(schema_with = "crate::wire::schema::user_address_id")]
        address_id: UserAddressId,
    },
    /// A country, optionally with a postal code.
    CountryPostal {
        /// ISO 3166-1 alpha-2 country code.
        country: String,
        /// Postal code within the country.
        #[serde(
            rename = "postalCode",
            default,
            skip_serializing_if = "Option::is_none"
        )]
        postal_code: Option<String>,
    },
}
impl DestinationData {
    pub fn into_core(self) -> Result<BrowsingDestination, InvalidDestinationData> {
        Ok(match self {
            Self::None {} => BrowsingDestination::None,
            Self::Address { address_id } => BrowsingDestination::Address(address_id),
            Self::CountryPostal {
                country,
                postal_code,
            } => BrowsingDestination::CountryPostal {
                country: country_from_code(&country, ReferenceRelease::CURRENT)
                    .map_err(|_| InvalidDestinationData)?,
                postal_code: postal_code
                    .map(PostalCode::new)
                    .transpose()
                    .map_err(|_| InvalidDestinationData)?,
            },
        })
    }
}
impl From<BrowsingDestination> for DestinationData {
    fn from(v: BrowsingDestination) -> Self {
        match v {
            BrowsingDestination::None => Self::None {},
            BrowsingDestination::Address(id) => Self::Address { address_id: id },
            BrowsingDestination::CountryPostal {
                country,
                postal_code,
            } => Self::CountryPostal {
                country: country.alpha2().into(),
                postal_code: postal_code.map(|p| p.as_str().to_owned()),
            },
        }
    }
}
#[derive(Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SetDestinationData {
    /// Current version from the last read; a stale version returns CONFLICT.
    #[schemars(range(min = 1, max = i64::MAX as u64))]
    pub expected_version: u64,
    /// The browsing destination to store, replacing the current one.
    pub destination: DestinationData,
}
#[derive(Serialize, JsonSchema)]
pub struct DestinationResponse {
    #[schemars(range(max = u64::MAX))]
    pub version: u64,
    pub destination: DestinationData,
}
impl From<DestinationView> for DestinationResponse {
    fn from(v: DestinationView) -> Self {
        Self {
            version: v.version,
            destination: v.destination.into(),
        }
    }
}
