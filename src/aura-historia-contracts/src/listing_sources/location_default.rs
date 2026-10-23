//! Protected source defaults retain applicability evidence and independent CAS revisions.
use crate::patch_value::{PatchValue, clearable};
use application::patch_field::PatchField;
use listing_source_core::{ListingSourceId, location_default::*};
use listing_source_service::{
    ports::location_default::LocationDefaultRevision, use_cases::location_default::*,
};
use party_core::party_location_id::PartyLocationId;
use schemars::{JsonSchema, Schema, SchemaGenerator, json_schema};
use serde::{Deserialize, Serialize};

#[derive(Debug, thiserror::Error)]
#[error("Invalid source location default input.")]
pub struct DefaultInputError;
fn deserialize_revision<'de, D: serde::Deserializer<'de>>(
    d: D,
) -> Result<LocationDefaultRevision, D::Error> {
    LocationDefaultRevision::try_from(i64::deserialize(d)?).map_err(serde::de::Error::custom)
}
fn revision_schema(_: &mut SchemaGenerator) -> Schema {
    json_schema!({"type":"integer","minimum":1,"maximum":i64::MAX})
}
fn evidence_scope_schema(_: &mut SchemaGenerator) -> Schema {
    let scopes = [
        SourceLocationMeaning::AssociatedBusiness,
        SourceLocationMeaning::AdvertisedInventory,
    ]
    .map(SourceLocationMeaning::as_str);
    json_schema!({"type":"string","enum":scopes})
}
fn eligibility_schema(_: &mut SchemaGenerator) -> Schema {
    json_schema!({"type":["string","null"],"enum":["ACTIVE","RETIRED",null]})
}
#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct EvidenceData {
    pub reference: String,
    #[schemars(schema_with = "evidence_scope_schema")]
    pub scope: String,
}
impl EvidenceData {
    pub fn into_core(self) -> Result<SourceLocationEvidence, DefaultInputError> {
        SourceLocationEvidence::new(
            self.reference,
            SourceLocationMeaning::from_code(&self.scope).ok_or(DefaultInputError)?,
        )
        .map_err(|_| DefaultInputError)
    }
}
impl From<&SourceLocationEvidence> for EvidenceData {
    fn from(e: &SourceLocationEvidence) -> Self {
        Self {
            reference: e.reference().to_owned(),
            scope: e.scope().as_str().to_owned(),
        }
    }
}
#[derive(Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AssignData {
    #[serde(deserialize_with = "deserialize_revision")]
    #[schemars(schema_with = "revision_schema")]
    pub expected_revision: LocationDefaultRevision,
    #[schemars(schema_with = "crate::wire::schema::party_location_id")]
    pub party_location_id: PartyLocationId,
    pub business_association: Option<EvidenceData>,
    pub advertised_inventory: Option<EvidenceData>,
}
impl AssignData {
    pub fn into_command(
        self,
        source_id: ListingSourceId,
    ) -> Result<AssignLocationDefaultCommand, DefaultInputError> {
        Ok(AssignLocationDefaultCommand {
            source_id,
            expected_revision: self.expected_revision,
            assignment: SourceLocationAssignment::new(
                self.party_location_id,
                self.business_association
                    .map(EvidenceData::into_core)
                    .transpose()?,
                self.advertised_inventory
                    .map(EvidenceData::into_core)
                    .transpose()?,
            )
            .map_err(|_| DefaultInputError)?,
        })
    }
}
#[derive(Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ClearData {
    #[serde(deserialize_with = "deserialize_revision")]
    #[schemars(schema_with = "revision_schema")]
    pub expected_revision: LocationDefaultRevision,
}
impl ClearData {
    pub fn into_command(self, source_id: ListingSourceId) -> ClearLocationDefaultCommand {
        ClearLocationDefaultCommand {
            source_id,
            expected_revision: self.expected_revision,
        }
    }
}
#[derive(Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ChangeData {
    #[serde(deserialize_with = "deserialize_revision")]
    #[schemars(schema_with = "revision_schema")]
    pub expected_revision: LocationDefaultRevision,
    #[serde(default)]
    pub business_association: PatchValue<EvidenceData>,
    #[serde(default)]
    pub advertised_inventory: PatchValue<EvidenceData>,
}
fn evidence_patch(
    value: PatchValue<EvidenceData>,
) -> Result<PatchField<SourceLocationEvidence>, DefaultInputError> {
    Ok(match clearable(value) {
        PatchField::Unchanged => PatchField::Unchanged,
        PatchField::Clear => PatchField::Clear,
        PatchField::Set(e) => PatchField::Set(e.into_core()?),
    })
}
impl ChangeData {
    pub fn into_command(
        self,
        source_id: ListingSourceId,
    ) -> Result<ChangeLocationDefaultApplicabilityCommand, DefaultInputError> {
        Ok(ChangeLocationDefaultApplicabilityCommand {
            source_id,
            expected_revision: self.expected_revision,
            business_association: evidence_patch(self.business_association)?,
            advertised_inventory: evidence_patch(self.advertised_inventory)?,
        })
    }
}
#[derive(Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct AssignmentData {
    #[schemars(schema_with = "crate::wire::schema::party_location_id")]
    pub party_location_id: PartyLocationId,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub business_association: Option<EvidenceData>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub advertised_inventory: Option<EvidenceData>,
}
#[derive(Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct DefaultData {
    #[schemars(schema_with = "crate::wire::schema::listing_source_id")]
    pub listing_source_id: ListingSourceId,
    #[schemars(schema_with = "revision_schema")]
    pub revision: LocationDefaultRevision,
    pub assignment: Option<AssignmentData>,
    #[schemars(schema_with = "eligibility_schema")]
    pub eligibility: Option<&'static str>,
}
impl From<LocationDefaultView> for DefaultData {
    fn from(v: LocationDefaultView) -> Self {
        Self {
            listing_source_id: v.source_id,
            revision: v.revision,
            assignment: v.assignment.map(|a| AssignmentData {
                party_location_id: a.location_id(),
                business_association: a.business_association().map(Into::into),
                advertised_inventory: a.advertised_inventory().map(Into::into),
            }),
            eligibility: v.eligibility.map(|e| match e {
                SelectedLocationEligibility::Active => "ACTIVE",
                SelectedLocationEligibility::Retired => "RETIRED",
            }),
        }
    }
}
