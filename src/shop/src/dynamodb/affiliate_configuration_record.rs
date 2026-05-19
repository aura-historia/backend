use crate::core::affiliate_configuration::AffiliateConfiguration;
use serde::{Deserialize, Serialize};

#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum AffiliateConfigurationRecord {
    Catawiki,
}

impl From<AffiliateConfigurationRecord> for AffiliateConfiguration {
    fn from(record: AffiliateConfigurationRecord) -> Self {
        match record {
            AffiliateConfigurationRecord::Catawiki => AffiliateConfiguration::Catawiki,
        }
    }
}

impl From<AffiliateConfiguration> for AffiliateConfigurationRecord {
    fn from(config: AffiliateConfiguration) -> Self {
        match config {
            AffiliateConfiguration::Catawiki => AffiliateConfigurationRecord::Catawiki,
        }
    }
}
