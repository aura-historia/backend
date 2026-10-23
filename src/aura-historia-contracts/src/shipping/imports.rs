//! Provenance shared by shipping management and listing delivery imports.
use application::error::{BoxError, box_error};
use schemars::{JsonSchema, Schema, SchemaGenerator};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use shipping_json::listing_delivery;

/// Shared public and queue protocol; timestamps always use RFC3339, never receipt time.
#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ImportData {
    #[schemars(schema_with = "integration_schema")]
    integration: Value,
    #[schemars(range(min = 1, max = i64::MAX))]
    sequence: u64,
    #[schemars(schema_with = "crate::wire::schema::datetime")]
    acquired_at: String,
    #[schemars(regex(pattern = "^[!-~]{1,128}$"))]
    request_key: String,
    #[serde(default)]
    #[schemars(regex(pattern = "^[!-~]{1,128}$"))]
    object_key: Option<String>,
    #[serde(default)]
    observation: Option<ObservationData>,
}
#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ObservationData {}
impl ImportData {
    pub fn into_command(self) -> Result<shipping_service::imports::ShippingImportWrite, BoxError> {
        use time::{OffsetDateTime, format_description::well_known::Rfc3339};
        let acquired_at = OffsetDateTime::parse(&self.acquired_at, &Rfc3339).map_err(box_error)?;
        let observation = self
            .observation
            .map(|_| shipping_service::imports::ShippingObservation { acquired_at });
        let write = shipping_service::imports::ShippingImportWrite {
            integration: listing_delivery::decode_integration(self.integration)?,
            sequence: self.sequence,
            acquired_at,
            request_key: self.request_key,
            object_key: self.object_key,
            observation,
        };
        write.validate().map_err(box_error)?;
        Ok(write)
    }
    pub fn from_command(
        w: &shipping_service::imports::ShippingImportWrite,
    ) -> Result<Self, BoxError> {
        let observation = w.observation.as_ref().map(|_| ObservationData {});
        Ok(Self {
            integration: listing_delivery::encode_integration(&w.integration)?,
            sequence: w.sequence,
            acquired_at: w
                .acquired_at
                .format(&time::format_description::well_known::Rfc3339)
                .map_err(box_error)?,
            request_key: w.request_key.clone(),
            object_key: w.object_key.clone(),
            observation,
        })
    }
}

/// Shared authoritative integration schema for shipping/listing import consumers.
pub fn integration_schema(generator: &mut SchemaGenerator) -> Schema {
    listing_delivery::integration_schema(generator)
}
