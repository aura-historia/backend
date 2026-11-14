use notification_core::external_email::{
    ExternalEmailContent, ExternalEmailTemplate, ExternalEmailTestProbe,
    InvalidExternalEmailTestProbe,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub(crate) const PAYLOAD_VERSION: i32 = 1;

#[derive(Debug, thiserror::Error)]
pub(crate) enum ExternalEmailMappingError {
    #[error("unknown external email template {0}")]
    UnknownTemplate(String),
    #[error("unsupported external email payload version {0}")]
    UnsupportedPayloadVersion(i32),
    #[error("external email payload serialization failed")]
    PayloadSerialization(#[source] serde_json::Error),
    #[error("external email payload is invalid")]
    InvalidPayload(#[source] serde_json::Error),
    #[error("external email test probe payload contains an invalid value")]
    InvalidTestProbe(#[source] InvalidExternalEmailTestProbe),
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct TestProbePayloadV1 {
    reference: String,
}

pub(crate) fn encode_payload(
    content: &ExternalEmailContent,
) -> Result<Value, ExternalEmailMappingError> {
    match content {
        ExternalEmailContent::TestProbe(probe) => serde_json::to_value(TestProbePayloadV1 {
            reference: probe.reference().to_owned(),
        }),
    }
    .map_err(ExternalEmailMappingError::PayloadSerialization)
}

pub(crate) fn decode_content(
    template: &str,
    payload_version: i32,
    payload: Value,
) -> Result<ExternalEmailContent, ExternalEmailMappingError> {
    let template = ExternalEmailTemplate::from_code(template)
        .ok_or_else(|| ExternalEmailMappingError::UnknownTemplate(template.to_owned()))?;
    if payload_version != PAYLOAD_VERSION {
        return Err(ExternalEmailMappingError::UnsupportedPayloadVersion(
            payload_version,
        ));
    }
    match template {
        ExternalEmailTemplate::TestProbe => {
            let payload = serde_json::from_value::<TestProbePayloadV1>(payload)
                .map_err(ExternalEmailMappingError::InvalidPayload)?;
            ExternalEmailTestProbe::new(&payload.reference)
                .map(ExternalEmailContent::TestProbe)
                .map_err(ExternalEmailMappingError::InvalidTestProbe)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn probe() -> ExternalEmailContent {
        ExternalEmailContent::TestProbe(
            ExternalEmailTestProbe::new("probe-1")
                .unwrap_or_else(|error| panic!("invalid test probe: {error}")),
        )
    }

    #[test]
    fn should_round_trip_test_probe_payload() -> Result<(), ExternalEmailMappingError> {
        let content = probe();
        let payload = encode_payload(&content)?;

        assert_eq!(json!({ "reference": "probe-1" }), payload);
        assert_eq!(
            content,
            decode_content("TEST_PROBE", PAYLOAD_VERSION, payload)?
        );
        Ok(())
    }

    #[test]
    fn should_reject_unknown_template() {
        assert!(matches!(
            decode_content(
                "UNKNOWN",
                PAYLOAD_VERSION,
                json!({ "reference": "probe-1" })
            ),
            Err(ExternalEmailMappingError::UnknownTemplate(_))
        ));
    }

    #[test]
    fn should_reject_unsupported_payload_version() {
        assert!(matches!(
            decode_content("TEST_PROBE", 2, json!({ "reference": "probe-1" })),
            Err(ExternalEmailMappingError::UnsupportedPayloadVersion(2))
        ));
    }

    #[test]
    fn should_reject_undecodable_payload() {
        for payload in [
            json!({}),
            json!({ "reference": 1 }),
            json!({ "reference": "probe-1", "extra": true }),
        ] {
            assert!(matches!(
                decode_content("TEST_PROBE", PAYLOAD_VERSION, payload),
                Err(ExternalEmailMappingError::InvalidPayload(_))
            ));
        }
    }

    #[test]
    fn should_reject_payload_that_violates_content_validation() {
        assert!(matches!(
            decode_content("TEST_PROBE", PAYLOAD_VERSION, json!({ "reference": " " })),
            Err(ExternalEmailMappingError::InvalidTestProbe(_))
        ));
    }
}
