use listing_source_core::ReferralConfiguration;

pub(crate) fn referral_configuration(
    value: Option<&serde_json::Value>,
) -> Result<Option<ReferralConfiguration>, ()> {
    value
        .map(|value| {
            match (
                value.get("kind").and_then(serde_json::Value::as_str),
                value.get("camref").and_then(serde_json::Value::as_str),
            ) {
                (Some("PARTNERIZE"), Some(camref)) => Ok(ReferralConfiguration::Partnerize {
                    camref: listing_source_core::PartnerizeCamref::try_from(camref)
                        .map_err(|_| ())?,
                }),
                _ => Err(()),
            }
        })
        .transpose()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_reject_unsafe_persisted_partnerize_camref() {
        for camref in [
            " campaign",
            "campaign/ref",
            "campaign?ref",
            "campaign#ref",
            "café",
        ] {
            let value = serde_json::json!({"kind":"PARTNERIZE","camref":camref});

            assert!(referral_configuration(Some(&value)).is_err(), "{camref:?}");
        }
    }
}
