use std::str::FromStr;

use strum::IntoEnumIterator;
use strum_macros::EnumIter;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, EnumIter)]
pub enum ListingIngestionMethod {
    WebCrawl,
    Shopify,
    Woocommerce,
    PartnerApi,
}

impl ListingIngestionMethod {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::WebCrawl => "WEB_CRAWL",
            Self::Shopify => "SHOPIFY",
            Self::Woocommerce => "WOOCOMMERCE",
            Self::PartnerApi => "PARTNER_API",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("invalid ingestion method '{value}'")]
pub struct InvalidListingIngestionMethod {
    value: String,
}

impl FromStr for ListingIngestionMethod {
    type Err = InvalidListingIngestionMethod;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::iter()
            .find(|method| method.as_str() == value)
            .ok_or_else(|| InvalidListingIngestionMethod {
                value: value.to_owned(),
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_parse_only_exact_ingestion_values() {
        assert_eq!(Ok(ListingIngestionMethod::WebCrawl), "WEB_CRAWL".parse());
        assert!("web_crawl".parse::<ListingIngestionMethod>().is_err());
    }
}
