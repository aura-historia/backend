//! Shipping profile/default wire contracts and pure use-case mappings shared by REST and MCP.
use super::terms;
use application::error::{BoxError, box_error};
use listing_source_core::ListingSourceId;
use party_core::party_id::PartyId;
use schemars::{JsonSchema, Schema, SchemaGenerator};
use serde::{Deserialize, Serialize};
use shipping_core::profile::{ShippingProfileContent, ShippingProfileId, ShippingProfileName};
use shipping_service::{
    ports::{ShippingDefaultRevision, ShippingEvidence, ShippingManagementRevision},
    use_cases::{
        AssignDefaultCommand, ClearDefaultCommand, CreateProfileCommand, DefaultEligibility,
        DefaultView, ProfileLifecycleCommand, ProfileSummary, ProfileView, ReplaceProfileCommand,
    },
};

pub struct RequiredNullable<T>(pub Option<T>);
impl<T: Serialize> Serialize for RequiredNullable<T> {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.0.serialize(serializer)
    }
}
fn required<'de, D: serde::Deserializer<'de>, T: Deserialize<'de>>(
    d: D,
) -> Result<RequiredNullable<T>, D::Error> {
    Option::deserialize(d).map(RequiredNullable)
}
#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ContentData {
    pub name: String,
    #[serde(deserialize_with = "required")]
    #[schemars(required, schema_with = "optional_party_id")]
    pub attributed_party_id: RequiredNullable<PartyId>,
    #[schemars(schema_with = "super::terms::schema")]
    pub terms: serde_json::Value,
}
impl ContentData {
    fn into_content(self, source: ListingSourceId) -> Result<ShippingProfileContent, BoxError> {
        Ok(ShippingProfileContent {
            name: ShippingProfileName::new(self.name).map_err(box_error)?,
            attributed_party: self.attributed_party_id.0,
            terms: terms::decode(self.terms, source)?,
        })
    }
}
#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct EvidenceData {
    pub reference: String,
    pub assertion: String,
}
fn evidence(data: Vec<EvidenceData>) -> Result<Vec<ShippingEvidence>, BoxError> {
    data.into_iter()
        .map(|e| ShippingEvidence::new(e.reference, e.assertion).map_err(box_error))
        .collect()
}
#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CreateData {
    #[serde(default)]
    pub import: Option<super::imports::ImportData>,
    #[schemars(regex(pattern = "^[!-~]{1,128}$"))]
    pub creation_key: String,
    pub content: ContentData,
    #[schemars(length(max = 32))]
    pub evidence: Vec<EvidenceData>,
}
#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReplaceData {
    #[serde(default)]
    pub import: Option<super::imports::ImportData>,
    #[schemars(schema_with = "revision")]
    pub expected_revision: ShippingManagementRevision,
    pub content: ContentData,
    #[schemars(length(max = 32))]
    pub evidence: Vec<EvidenceData>,
}
#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LifecycleData {
    #[serde(default)]
    pub import: Option<super::imports::ImportData>,
    #[schemars(schema_with = "revision")]
    pub expected_revision: ShippingManagementRevision,
}
#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AssignData {
    #[serde(default)]
    pub import: Option<super::imports::ImportData>,
    #[schemars(schema_with = "revision")]
    pub expected_revision: ShippingDefaultRevision,
    #[schemars(schema_with = "profile_id")]
    pub shipping_profile_id: ShippingProfileId,
}
#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ClearData {
    #[serde(default)]
    pub import: Option<super::imports::ImportData>,
    #[schemars(schema_with = "revision")]
    pub expected_revision: ShippingDefaultRevision,
}
#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ListQuery {
    #[serde(default)]
    pub object_key: Option<String>,
    #[serde(default)]
    #[schemars(schema_with = "optional_provider_method")]
    pub integration_method: Option<String>,
    #[serde(default)]
    #[schemars(schema_with = "optional_profile_id")]
    pub after: Option<ShippingProfileId>,
    #[serde(default)]
    #[schemars(range(min = 1, max = 100))]
    pub size: Option<u32>,
}
#[derive(Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ProfileData {
    #[schemars(schema_with = "profile_id")]
    pub shipping_profile_id: ShippingProfileId,
    #[schemars(schema_with = "source_id")]
    pub listing_source_id: ListingSourceId,
    #[schemars(schema_with = "revision")]
    pub management_revision: ShippingManagementRevision,
    #[schemars(range(min = 1, max = u64::MAX))]
    pub terms_revision: u64,
    #[schemars(schema_with = "lifecycle")]
    pub lifecycle: &'static str,
    pub content: ProfileContentData,
}
#[derive(Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ProfileContentData {
    pub name: String,
    #[schemars(schema_with = "optional_party_id")]
    pub attributed_party_id: Option<PartyId>,
    #[schemars(schema_with = "super::terms::schema")]
    pub terms: serde_json::Value,
}
impl TryFrom<ProfileView> for ProfileData {
    type Error = BoxError;
    fn try_from(v: ProfileView) -> Result<Self, Self::Error> {
        Ok(Self {
            shipping_profile_id: v.id,
            listing_source_id: v.source_id,
            management_revision: v.management_revision,
            terms_revision: v.revision.into_inner(),
            lifecycle: v.lifecycle.as_str(),
            content: ProfileContentData {
                name: v.content.name.as_str().into(),
                attributed_party_id: v.content.attributed_party,
                terms: terms::encode(&v.content.terms)?,
            },
        })
    }
}
#[derive(Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct SummaryData {
    #[schemars(schema_with = "profile_id")]
    pub shipping_profile_id: ShippingProfileId,
    #[schemars(schema_with = "source_id")]
    pub listing_source_id: ListingSourceId,
    pub name: String,
    #[schemars(schema_with = "revision")]
    pub management_revision: ShippingManagementRevision,
    #[schemars(range(min = 1, max = u64::MAX))]
    pub terms_revision: u64,
    #[schemars(schema_with = "lifecycle")]
    pub lifecycle: &'static str,
}
impl From<ProfileSummary> for SummaryData {
    fn from(v: ProfileSummary) -> Self {
        Self {
            shipping_profile_id: v.id,
            listing_source_id: v.source_id,
            name: v.name.as_str().into(),
            management_revision: v.management_revision,
            terms_revision: v.revision.into_inner(),
            lifecycle: v.lifecycle.as_str(),
        }
    }
}
#[derive(Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct DefaultData {
    #[schemars(schema_with = "source_id")]
    pub listing_source_id: ListingSourceId,
    #[schemars(schema_with = "revision")]
    pub revision: ShippingDefaultRevision,
    #[schemars(schema_with = "optional_profile_id")]
    pub shipping_profile_id: Option<ShippingProfileId>,
    #[schemars(schema_with = "eligibility")]
    pub eligibility: Option<&'static str>,
}
impl From<DefaultView> for DefaultData {
    fn from(v: DefaultView) -> Self {
        Self {
            listing_source_id: v.source_id,
            revision: v.revision,
            shipping_profile_id: v.profile_id,
            eligibility: v.eligibility.map(|e| match e {
                DefaultEligibility::Active => "ACTIVE",
                DefaultEligibility::Retired => "RETIRED",
                DefaultEligibility::Missing => "MISSING",
            }),
        }
    }
}

pub fn profile_id(_: &mut SchemaGenerator) -> Schema {
    crate::wire::schema::object_id(ShippingProfileId::PREFIX)
}
pub fn source_id(generator: &mut SchemaGenerator) -> Schema {
    crate::wire::schema::listing_source_id(generator)
}
pub fn optional_profile_id(generator: &mut SchemaGenerator) -> Schema {
    schemars::json_schema!({"anyOf":[profile_id(generator),{"type":"null"}]})
}
fn optional_party_id(_: &mut SchemaGenerator) -> Schema {
    schemars::json_schema!({"anyOf":[crate::wire::schema::object_id(PartyId::PREFIX),{"type":"null"}]})
}
fn revision(_: &mut SchemaGenerator) -> Schema {
    schemars::json_schema!({"type":"integer","minimum":1,"maximum":u64::MAX})
}
fn lifecycle(_: &mut SchemaGenerator) -> Schema {
    schemars::json_schema!({"type":"string","enum":["ACTIVE","RETIRED"]})
}
fn eligibility(_: &mut SchemaGenerator) -> Schema {
    schemars::json_schema!({"anyOf":[{"type":"string","enum":["ACTIVE","RETIRED","MISSING"]},{"type":"null"}]})
}

impl CreateData {
    pub fn into_command(
        self,
        source_id: ListingSourceId,
    ) -> Result<CreateProfileCommand, BoxError> {
        Ok(CreateProfileCommand {
            source_id,
            import: self
                .import
                .map(super::imports::ImportData::into_command)
                .transpose()?,
            creation_key: self.creation_key,
            content: self.content.into_content(source_id)?,
            evidence: evidence(self.evidence)?,
        })
    }
}
impl ReplaceData {
    pub fn into_command(
        self,
        source_id: ListingSourceId,
        profile_id: ShippingProfileId,
    ) -> Result<ReplaceProfileCommand, BoxError> {
        Ok(ReplaceProfileCommand {
            source_id,
            profile_id,
            import: self
                .import
                .map(super::imports::ImportData::into_command)
                .transpose()?,
            expected_revision: self.expected_revision,
            content: self.content.into_content(source_id)?,
            evidence: evidence(self.evidence)?,
        })
    }
}
impl LifecycleData {
    pub fn into_command(
        self,
        source_id: ListingSourceId,
        profile_id: ShippingProfileId,
    ) -> Result<ProfileLifecycleCommand, BoxError> {
        Ok(ProfileLifecycleCommand {
            source_id,
            profile_id,
            import: self
                .import
                .map(super::imports::ImportData::into_command)
                .transpose()?,
            expected_revision: self.expected_revision,
        })
    }
}
impl AssignData {
    pub fn into_command(
        self,
        source_id: ListingSourceId,
    ) -> Result<AssignDefaultCommand, BoxError> {
        Ok(AssignDefaultCommand {
            source_id,
            profile_id: self.shipping_profile_id,
            import: self
                .import
                .map(super::imports::ImportData::into_command)
                .transpose()?,
            expected_revision: self.expected_revision,
        })
    }
}
impl ClearData {
    pub fn into_command(self, source_id: ListingSourceId) -> Result<ClearDefaultCommand, BoxError> {
        Ok(ClearDefaultCommand {
            source_id,
            import: self
                .import
                .map(super::imports::ImportData::into_command)
                .transpose()?,
            expected_revision: self.expected_revision,
        })
    }
}
impl ListQuery {
    pub fn into_identity(
        self,
    ) -> Result<Option<shipping_service::imports::ProfileImportIdentity>, BoxError> {
        match (self.object_key, self.integration_method) {
            (None, None) => Ok(None),
            (Some(object_key), method) => {
                let integration = match method {
                    None => product_listing_core::delivery::DeliveryIntegration::Source,
                    Some(method) => shipping_json::listing_delivery::decode_integration(
                        serde_json::json!({"kind":"PROVIDER","method":method}),
                    )?,
                };
                Ok(Some(shipping_service::imports::ProfileImportIdentity {
                    object_key,
                    integration,
                }))
            }
            _ => Err(application::error::static_error(
                "integrationMethod requires objectKey",
            )),
        }
    }
}

fn optional_provider_method(generator: &mut SchemaGenerator) -> Schema {
    schemars::json_schema!({"anyOf":[shipping_json::listing_delivery::provider_method_schema(generator),{"type":"null"}]})
}
