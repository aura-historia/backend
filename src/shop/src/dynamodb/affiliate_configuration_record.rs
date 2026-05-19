use crate::core::affiliate_configuration::AffiliateConfiguration;

/// Flat DynamoDB representation of affiliate configuration.
/// Uses `affiliate_configuration_type` to discriminate the variant and
/// variant-specific optional columns for parameters.
#[derive(Debug, Clone, PartialEq)]
pub struct AffiliateConfigurationRecord {
    pub affiliate_configuration_type: String,
    pub affiliate_configuration_partnerize_camref: Option<String>,
}

const TYPE_PARTNERIZE: &str = "PARTNERIZE";

impl From<AffiliateConfiguration> for AffiliateConfigurationRecord {
    fn from(config: AffiliateConfiguration) -> Self {
        match config {
            AffiliateConfiguration::Partnerize { camref } => AffiliateConfigurationRecord {
                affiliate_configuration_type: TYPE_PARTNERIZE.to_string(),
                affiliate_configuration_partnerize_camref: Some(camref),
            },
        }
    }
}

impl TryFrom<AffiliateConfigurationRecord> for AffiliateConfiguration {
    type Error = String;

    fn try_from(record: AffiliateConfigurationRecord) -> Result<Self, Self::Error> {
        match record.affiliate_configuration_type.as_str() {
            TYPE_PARTNERIZE => Ok(AffiliateConfiguration::Partnerize {
                camref: record
                    .affiliate_configuration_partnerize_camref
                    .ok_or_else(|| {
                        "Missing affiliate_configuration_partnerize_camref for PARTNERIZE"
                            .to_string()
                    })?,
            }),
            other => Err(format!("Unknown affiliate_configuration_type: {other}")),
        }
    }
}
