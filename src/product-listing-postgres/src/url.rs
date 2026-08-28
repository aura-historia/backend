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
                    camref: camref.to_owned(),
                }),
                _ => Err(()),
            }
        })
        .transpose()
}
