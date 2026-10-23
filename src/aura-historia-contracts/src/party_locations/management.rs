//! Protected management contracts. Public discovery never returns this evidence or revisions.
use crate::{
    geography::{GeographyData, PositionData},
    patch_value::{PatchValue, clearable},
};
use application::patch_field::PatchField;
use party_core::{
    party_id::PartyId,
    party_location::{
        PartyLocationContent, PartyLocationLabel, PartyLocationLifecycle, PartyLocationRole,
    },
    party_location_id::PartyLocationId,
};
use party_service::{
    ports::party_location::{ExpectedPartyLocation, PartyLocationRevision},
    use_cases::party_locations::*,
};
use schemars::{JsonSchema, Schema, SchemaGenerator, json_schema};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use time::OffsetDateTime;

#[derive(Debug, thiserror::Error)]
pub enum LocationInputError {
    #[error("Invalid PartyLocation input.")]
    Invalid,
    #[error("Body field '{0}' must not be null.")]
    Null(&'static str),
}
use LocationInputError::Invalid;

fn map_patch<T, U>(
    value: PatchField<T>,
    convert: impl FnOnce(T) -> Result<U, LocationInputError>,
) -> Result<PatchField<U>, LocationInputError> {
    Ok(match value {
        PatchField::Unchanged => PatchField::Unchanged,
        PatchField::Clear => PatchField::Clear,
        PatchField::Set(value) => PatchField::Set(convert(value)?),
    })
}
fn required<T>(
    value: PatchValue<T>,
    field: &'static str,
) -> Result<PatchField<T>, LocationInputError> {
    match value {
        PatchValue::Omitted => Ok(PatchField::Unchanged),
        PatchValue::Null => Err(LocationInputError::Null(field)),
        PatchValue::Value(value) => Ok(PatchField::Set(value)),
    }
}
fn revision_schema(_: &mut SchemaGenerator) -> Schema {
    json_schema!({"type":"integer","minimum":1,"maximum":u64::MAX})
}
fn party_id_schema(_: &mut SchemaGenerator) -> Schema {
    crate::wire::schema::object_id(PartyId::PREFIX)
}

fn roles_schema(_: &mut SchemaGenerator) -> Schema {
    let roles = [
        PartyLocationRole::BusinessPremises,
        PartyLocationRole::Warehouse,
        PartyLocationRole::RegisteredAddress,
        PartyLocationRole::Correspondence,
    ]
    .map(PartyLocationRole::as_str);
    json_schema!({"type":"array","maxItems":4,"uniqueItems":true,"items":{"type":"string","enum":roles}})
}
fn evidence_scope_schema(_: &mut SchemaGenerator) -> Schema {
    let scopes = [
        LocationAssertionScope::SiteDescription,
        LocationAssertionScope::RegisteredAddress,
        LocationAssertionScope::CorrespondenceAddress,
    ]
    .map(LocationAssertionScope::as_str);
    json_schema!({"type":"string","enum":scopes})
}
fn lifecycle_schema(_: &mut SchemaGenerator) -> Schema {
    let lifecycles = [
        PartyLocationLifecycle::Active,
        PartyLocationLifecycle::Retired,
    ]
    .map(PartyLocationLifecycle::as_str);
    json_schema!({"type":"string","enum":lifecycles})
}
fn correction_schema(_: &mut SchemaGenerator) -> Schema {
    json_schema!({"type":"string","enum":["SAME_SITE"]})
}

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EvidenceData {
    pub reference: String,
    #[serde(
        default,
        with = "time::serde::rfc3339::option",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(schema_with = "crate::wire::schema::optional_datetime")]
    pub observed_at: Option<OffsetDateTime>,
    #[schemars(schema_with = "evidence_scope_schema")]
    pub scope: String,
}
impl EvidenceData {
    pub fn into_service(self) -> Result<LocationEvidence, LocationInputError> {
        LocationEvidence::new(
            self.reference,
            self.observed_at,
            LocationAssertionScope::from_code(&self.scope).ok_or(Invalid)?,
        )
        .map_err(|_| Invalid)
    }
}
impl From<LocationEvidence> for EvidenceData {
    fn from(v: LocationEvidence) -> Self {
        Self {
            reference: v.reference().to_owned(),
            observed_at: v.observed_at(),
            scope: v.scope().as_str().to_owned(),
        }
    }
}
#[derive(Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct LocationData {
    #[schemars(schema_with = "crate::wire::schema::party_location_id")]
    pub id: PartyLocationId,
    #[schemars(schema_with = "party_id_schema")]
    pub party_id: PartyId,
    pub label: String,
    #[schemars(schema_with = "roles_schema")]
    pub roles: Vec<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub geography: Option<GeographyData>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub position: Option<PositionData>,
    #[schemars(schema_with = "lifecycle_schema")]
    pub lifecycle: &'static str,
    #[schemars(schema_with = "revision_schema")]
    pub revision: PartyLocationRevision,
    #[schemars(range(max = u64::MAX))]
    pub input_revision: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub evidence: Option<EvidenceData>,
}
impl From<PartyLocationView> for LocationData {
    fn from(v: PartyLocationView) -> Self {
        Self {
            id: v.id,
            party_id: v.party_id,
            label: v.label.as_str().to_owned(),
            roles: v.roles.iter().map(|v| v.as_str()).collect(),
            geography: v.geography.map(Into::into),
            position: v.position.map(Into::into),
            lifecycle: v.lifecycle.as_str(),
            revision: v.revision,
            input_revision: v.input_revision,
            evidence: v.evidence.map(Into::into),
        }
    }
}
fn role_set(roles: Vec<String>) -> Result<BTreeSet<PartyLocationRole>, LocationInputError> {
    if roles.len() > 4 {
        return Err(Invalid);
    }
    let count = roles.len();
    let set = roles
        .into_iter()
        .map(|v| PartyLocationRole::from_code(&v).ok_or(Invalid))
        .collect::<Result<BTreeSet<_>, _>>()?;
    if count != set.len() {
        return Err(Invalid);
    }
    Ok(set)
}
#[derive(Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ExpectedData {
    #[schemars(schema_with = "crate::wire::schema::party_location_id")]
    pub id: PartyLocationId,
    #[schemars(schema_with = "revision_schema")]
    pub revision: PartyLocationRevision,
}
impl From<ExpectedData> for ExpectedPartyLocation {
    fn from(v: ExpectedData) -> Self {
        Self {
            id: v.id,
            revision: v.revision,
        }
    }
}
#[derive(Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CreateData {
    pub label: String,
    #[serde(default)]
    #[schemars(schema_with = "roles_schema")]
    pub roles: Vec<String>,
    #[serde(default)]
    pub geography: Option<GeographyData>,
    #[serde(default)]
    pub position: Option<PositionData>,
    #[serde(default)]
    pub evidence: Option<EvidenceData>,
    #[serde(default)]
    pub relocates: Option<ExpectedData>,
}
impl CreateData {
    pub fn into_command(
        self,
        party_id: PartyId,
    ) -> Result<CreatePartyLocationCommand, LocationInputError> {
        Ok(CreatePartyLocationCommand {
            party_id,
            content: PartyLocationContent {
                label: PartyLocationLabel::new(self.label).map_err(|_| Invalid)?,
                roles: role_set(self.roles)?,
                geography: self
                    .geography
                    .map(GeographyData::into_core)
                    .transpose()
                    .map_err(|_| Invalid)?,
                position: self
                    .position
                    .map(PositionData::into_core)
                    .transpose()
                    .map_err(|_| Invalid)?,
            },
            evidence: self.evidence.map(EvidenceData::into_service).transpose()?,
            relocates: self.relocates.map(Into::into),
        })
    }
}
#[derive(Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UpdateData {
    #[schemars(schema_with = "revision_schema")]
    pub expected_revision: PartyLocationRevision,
    /// SAME_SITE corrects this site; relocation is a create with relocates.
    #[schemars(schema_with = "correction_schema")]
    pub correction: String,
    #[serde(default)]
    #[schemars(schema_with = "String::json_schema")]
    pub label: PatchValue<String>,
    #[serde(default)]
    #[schemars(schema_with = "roles_schema")]
    pub roles: PatchValue<Vec<String>>,
    #[serde(default)]
    pub geography: PatchValue<GeographyData>,
    #[serde(default)]
    pub position: PatchValue<PositionData>,
    #[serde(default)]
    pub evidence: PatchValue<EvidenceData>,
}
impl UpdateData {
    pub fn into_command(
        self,
        party_id: PartyId,
        id: PartyLocationId,
    ) -> Result<UpdatePartyLocationCommand, LocationInputError> {
        if self.correction != "SAME_SITE" {
            return Err(Invalid);
        }
        Ok(UpdatePartyLocationCommand {
            party_id,
            expected: ExpectedPartyLocation {
                id,
                revision: self.expected_revision,
            },
            label: map_patch(required(self.label, "label")?, |v| {
                PartyLocationLabel::new(v).map_err(|_| Invalid)
            })?,
            roles: map_patch(required(self.roles, "roles")?, role_set)?,
            geography: map_patch(clearable(self.geography), |v| {
                v.into_core().map_err(|_| Invalid)
            })?,
            position: map_patch(clearable(self.position), |v| {
                v.into_core().map_err(|_| Invalid)
            })?,
            evidence: map_patch(clearable(self.evidence), EvidenceData::into_service)?,
        })
    }
}
#[derive(Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LifecycleData {
    #[schemars(schema_with = "revision_schema")]
    pub expected_revision: PartyLocationRevision,
    #[schemars(schema_with = "lifecycle_schema")]
    pub lifecycle: String,
}
impl LifecycleData {
    pub fn into_command(
        self,
        party_id: PartyId,
        id: PartyLocationId,
    ) -> Result<SetPartyLocationLifecycleCommand, LocationInputError> {
        Ok(SetPartyLocationLifecycleCommand {
            party_id,
            expected: ExpectedPartyLocation {
                id,
                revision: self.expected_revision,
            },
            lifecycle: PartyLocationLifecycle::from_code(&self.lifecycle).ok_or(Invalid)?,
        })
    }
}
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ListData {
    #[serde(default)]
    #[schemars(schema_with = "crate::wire::schema::optional_party_location_id")]
    pub after: Option<String>,
    #[schemars(range(min = 1, max = 100))]
    pub limit: Option<u32>,
}
#[derive(Debug, thiserror::Error)]
pub enum ListInputError {
    #[error("Location page size must be from 1 through 100.")]
    Limit,
    #[error("must be a valid PartyLocation ID")]
    After(#[source] domain_primitives::object_id::ObjectIdError),
}
impl ListData {
    pub fn into_request(
        self,
        party_id: PartyId,
    ) -> Result<ListPartyLocationsRequest, ListInputError> {
        let limit = self.limit.unwrap_or(25);
        if !(1..=100).contains(&limit) {
            return Err(ListInputError::Limit);
        }
        Ok(ListPartyLocationsRequest {
            party_id,
            after: self
                .after
                .map(|v| v.parse().map_err(ListInputError::After))
                .transpose()?,
            limit,
        })
    }
}
#[derive(Serialize, JsonSchema)]
pub struct PageData {
    pub items: Vec<LocationData>,
    #[schemars(schema_with = "crate::wire::schema::optional_party_location_id")]
    pub next: Option<PartyLocationId>,
}
impl From<PartyLocationsPage> for PageData {
    fn from(v: PartyLocationsPage) -> Self {
        Self {
            items: v.items.into_iter().map(Into::into).collect(),
            next: v.next,
        }
    }
}
