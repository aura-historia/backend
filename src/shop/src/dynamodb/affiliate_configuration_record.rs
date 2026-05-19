use crate::core::affiliate_configuration::AffiliateConfiguration;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "SCREAMING_SNAKE_CASE")]
pub enum AffiliateConfigurationRecord {
    Partnerize { camref: String },
}

impl From<AffiliateConfiguration> for AffiliateConfigurationRecord {
    fn from(config: AffiliateConfiguration) -> Self {
        match config {
            AffiliateConfiguration::Partnerize { camref } => {
                AffiliateConfigurationRecord::Partnerize { camref }
            }
        }
    }
}

impl From<AffiliateConfigurationRecord> for AffiliateConfiguration {
    fn from(record: AffiliateConfigurationRecord) -> Self {
        match record {
            AffiliateConfigurationRecord::Partnerize { camref } => {
                AffiliateConfiguration::Partnerize { camref }
            }
        }
    }
}
