use common::year::Year;
use product::dynamodb::{
    authenticity_record::AuthenticityRecord, condition_record::ConditionRecord,
    provenance_record::ProvenanceRecord, restoration_record::RestorationRecord,
};
use serde::{Deserialize, Serialize};

/// Extracted antique attributes returned by Gemini.
///
/// Each field is optional; `None` means the model could not determine the
/// value from the product text.  Short JSON key names are intentional and
/// reduce prompt and response token counts.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct ExtractedAttributes {
    /// Exact origin year.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub y: Option<Year>,
    /// Lower bound of the origin year range.
    #[serde(rename = "yMin", skip_serializing_if = "Option::is_none", default)]
    pub y_min: Option<Year>,
    /// Upper bound of the origin year range.
    #[serde(rename = "yMax", skip_serializing_if = "Option::is_none", default)]
    pub y_max: Option<Year>,
    /// Authenticity of the antique.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub auth: Option<AuthenticityRecord>,
    /// Physical condition of the antique.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub cond: Option<ConditionRecord>,
    /// Provenance documentation.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub prov: Option<ProvenanceRecord>,
    /// Restoration work done.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub rest: Option<RestorationRecord>,
    /// Whether the item is from or related to Nazi Germany / SA / SS.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub nazi: Option<bool>,
}
