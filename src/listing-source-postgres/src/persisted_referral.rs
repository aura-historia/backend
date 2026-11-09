//! Persisted JSON shape of a ListingSource referral configuration.
//!
//! Every reader and the repository of this crate go through this module so the stored shape is
//! written and validated in one place. Invalid persisted values fail closed.

use listing_source_core::{PartnerizeCamref, PartnerizeCamrefError, ReferralConfiguration};

#[derive(Debug, thiserror::Error)]
pub(crate) enum PersistedReferralConfigurationError {
    #[error("persisted referral configuration kind is missing or unknown")]
    UnknownKind,
    #[error("persisted Partnerize referral configuration has no camref")]
    MissingCamref,
    #[error("persisted Partnerize referral configuration has an invalid camref")]
    InvalidCamref(#[source] PartnerizeCamrefError),
}

pub(crate) fn to_json(value: Option<&ReferralConfiguration>) -> Option<serde_json::Value> {
    value.map(|value| match value {
        ReferralConfiguration::Partnerize { camref } => {
            serde_json::json!({"kind":"PARTNERIZE","camref":camref.as_ref()})
        }
    })
}

pub(crate) fn from_json(
    value: Option<&serde_json::Value>,
) -> Result<Option<ReferralConfiguration>, PersistedReferralConfigurationError> {
    value.map(parse).transpose()
}

fn parse(
    value: &serde_json::Value,
) -> Result<ReferralConfiguration, PersistedReferralConfigurationError> {
    if value.get("kind").and_then(serde_json::Value::as_str) != Some("PARTNERIZE") {
        return Err(PersistedReferralConfigurationError::UnknownKind);
    }
    let camref = value
        .get("camref")
        .and_then(serde_json::Value::as_str)
        .ok_or(PersistedReferralConfigurationError::MissingCamref)?;

    Ok(ReferralConfiguration::Partnerize {
        camref: PartnerizeCamref::try_from(camref)
            .map_err(PersistedReferralConfigurationError::InvalidCamref)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_parse_safe_partnerize_referral_summary() {
        let value = serde_json::json!({
            "kind": "PARTNERIZE",
            "camref": "campaign123",
        });

        assert!(matches!(
            from_json(Some(&value)),
            Ok(Some(ReferralConfiguration::Partnerize { camref }))
                if camref.as_ref() == "campaign123"
        ));
    }

    #[test]
    fn should_treat_absent_referral_configuration_as_none() {
        assert!(matches!(from_json(None), Ok(None)));
    }

    #[test]
    fn should_reject_invalid_partnerize_referral_summary() {
        for value in [
            serde_json::json!({"kind":"OTHER","camref":"campaign123"}),
            serde_json::json!({"kind":"PARTNERIZE"}),
        ] {
            assert!(from_json(Some(&value)).is_err(), "{value}");
        }
    }

    #[test]
    fn should_reject_unsafe_partnerize_camref_from_persisted_configuration() {
        for camref in [" campaign", "campaign/ref", "campaign?ref", "café"] {
            let persisted = serde_json::json!({"kind":"PARTNERIZE","camref":camref});

            assert!(from_json(Some(&persisted)).is_err(), "{camref:?}");
        }
    }

    #[test]
    fn should_read_back_what_it_writes() {
        let camref = PartnerizeCamref::try_from("1101l3AbC")
            .unwrap_or_else(|error| panic!("test camref: {error}"));
        let configuration = ReferralConfiguration::Partnerize { camref };

        let written = to_json(Some(&configuration));

        assert!(matches!(
            from_json(written.as_ref()),
            Ok(Some(read)) if read == configuration
        ));
    }

    #[test]
    fn should_serialize_valid_partnerize_camref() {
        let camref = PartnerizeCamref::try_from("1101l3AbC")
            .unwrap_or_else(|error| panic!("test camref: {error}"));
        let configuration = ReferralConfiguration::Partnerize { camref };

        assert_eq!(
            Some(serde_json::json!({"kind":"PARTNERIZE","camref":"1101l3AbC"})),
            to_json(Some(&configuration))
        );
    }
}
