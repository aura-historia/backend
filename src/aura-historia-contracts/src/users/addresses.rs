use crate::geography::{GeographyData, PositionData};
use geo::DerivedGeography;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use user_core::user_address::UserAddressId;
use user_service::address_book::{
    CreateUserAddress, UpdateUserAddress, UserAddressPage, UserAddressResolution, UserAddressView,
};

#[derive(Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CreateData {
    pub label: String,
    pub geography: GeographyData,
}
#[derive(Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UpdateData {
    #[schemars(range(min = 1, max = i64::MAX as u64))]
    pub expected_version: u64,
    pub label: String,
    pub geography: GeographyData,
}
#[derive(Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ExpectedData {
    #[schemars(range(min = 1, max = i64::MAX as u64))]
    pub expected_version: u64,
}
#[derive(Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct AddressData {
    #[schemars(schema_with = "crate::wire::schema::user_address_id")]
    pub address_id: UserAddressId,
    pub label: String,
    pub geography: GeographyData,
    pub input_revision: u64,
    pub version: u64,
    pub preferred: bool,
    pub resolution: AddressResolutionData,
}
impl From<UserAddressView> for AddressData {
    fn from(v: UserAddressView) -> Self {
        Self {
            address_id: v.id,
            label: v.label.as_str().into(),
            geography: v.geography.into(),
            input_revision: v.input_revision,
            version: v.version,
            preferred: v.preferred,
            resolution: v.resolution.into(),
        }
    }
}
#[derive(Deserialize, Default, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PageQuery {
    #[serde(default)]
    #[schemars(schema_with = "crate::wire::schema::optional_user_address_id")]
    pub after: Option<UserAddressId>,
    #[serde(default)]
    #[schemars(range(min = 1, max = 100))]
    pub limit: Option<u32>,
}
#[derive(Serialize, JsonSchema)]
pub struct PageData {
    pub items: Vec<AddressData>,
    #[schemars(schema_with = "crate::wire::schema::optional_user_address_id")]
    pub next: Option<UserAddressId>,
}

#[derive(Serialize, JsonSchema)]
pub struct DerivedData {
    pub geography: Option<GeographyData>,
    pub position: Option<PositionData>,
}
impl From<DerivedGeography> for DerivedData {
    fn from(v: DerivedGeography) -> Self {
        Self {
            geography: v.description().cloned().map(Into::into),
            position: v.position().map(Into::into),
        }
    }
}
#[derive(Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct AddressAttributionData {
    pub provider: String,
    pub attribution_url: String,
}
#[derive(Serialize, JsonSchema)]
pub struct AddressResolutionData {
    pub status: &'static str,
    pub derived: Option<DerivedData>,
    pub attribution: Option<AddressAttributionData>,
}
impl From<UserAddressResolution> for AddressResolutionData {
    fn from(v: UserAddressResolution) -> Self {
        Self {
            status: v.status.as_str(),
            derived: v.derived.map(Into::into),
            attribution: v.attribution.map(|a| AddressAttributionData {
                provider: a.provider,
                attribution_url: a.attribution_url,
            }),
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum InvalidAddressData {
    #[error("invalid address label")]
    Label(#[source] user_core::user_address::InvalidUserAddress),
    #[error("invalid address geography")]
    Geography(#[from] crate::geography::InvalidGeographyData),
}
impl CreateData {
    pub fn into_command(self) -> Result<CreateUserAddress, InvalidAddressData> {
        Ok(CreateUserAddress {
            label: user_core::user_address::UserAddressLabel::new(self.label)
                .map_err(InvalidAddressData::Label)?,
            geography: self.geography.into_core()?,
        })
    }
}
impl UpdateData {
    pub fn into_command(self, id: UserAddressId) -> Result<UpdateUserAddress, InvalidAddressData> {
        Ok(UpdateUserAddress {
            id,
            expected_version: self.expected_version,
            label: user_core::user_address::UserAddressLabel::new(self.label)
                .map_err(InvalidAddressData::Label)?,
            geography: self.geography.into_core()?,
        })
    }
}
impl From<UserAddressPage> for PageData {
    fn from(view: UserAddressPage) -> Self {
        Self {
            items: view.items.into_iter().map(Into::into).collect(),
            next: view.next,
        }
    }
}
