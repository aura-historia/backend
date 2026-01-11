use common::year::Year;
use product::dynamodb::{
    authenticity_record::AuthenticityRecord, condition_record::ConditionRecord,
    provenance_record::ProvenanceRecord, restoration_record::RestorationRecord,
};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExtractedAttributes {
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub origin_year_min: Option<Year>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub origin_year: Option<Year>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub origin_year_max: Option<Year>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub authenticity: Option<AuthenticityRecord>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub condition: Option<ConditionRecord>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub provenance: Option<ProvenanceRecord>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub restoration: Option<RestorationRecord>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub is_from_nazi_germany_epoch: Option<bool>,
}
