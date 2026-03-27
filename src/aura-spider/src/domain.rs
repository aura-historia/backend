use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LinkClass {
    Product,
    Category,
    Imprint,
    Info,
    Other,
}

impl std::fmt::Display for LinkClass {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LinkClass::Product => write!(f, "product"),
            LinkClass::Category => write!(f, "category"),
            LinkClass::Imprint => write!(f, "imprint"),
            LinkClass::Info => write!(f, "info"),
            LinkClass::Other => write!(f, "other"),
        }
    }
}

impl std::str::FromStr for LinkClass {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "product" => Ok(LinkClass::Product),
            "category" => Ok(LinkClass::Category),
            "imprint" => Ok(LinkClass::Imprint),
            "info" => Ok(LinkClass::Info),
            "other" => Ok(LinkClass::Other),
            _ => Err(format!("Invalid link class: {}", s)),
        }
    }
}

impl LinkClass {
    pub fn as_str(self) -> &'static str {
        match self {
            LinkClass::Product => "product",
            LinkClass::Category => "category",
            LinkClass::Imprint => "imprint",
            LinkClass::Info => "info",
            LinkClass::Other => "other",
        }
    }

    pub fn from_db(value: &str) -> Self {
        match value {
            "product" => LinkClass::Product,
            "category" => LinkClass::Category,
            "imprint" => LinkClass::Imprint,
            "info" => LinkClass::Info,
            _ => LinkClass::Other,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum LinkState {
    Listed,
    Available,
    Reserved,
    Sold,
    Removed,
    Unknown,
}

impl std::fmt::Display for LinkState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LinkState::Listed => write!(f, "LISTED"),
            LinkState::Available => write!(f, "AVAILABLE"),
            LinkState::Reserved => write!(f, "RESERVED"),
            LinkState::Sold => write!(f, "SOLD"),
            LinkState::Removed => write!(f, "REMOVED"),
            LinkState::Unknown => write!(f, "UNKNOWN"),
        }
    }
}

impl std::str::FromStr for LinkState {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "LISTED" => Ok(LinkState::Listed),
            "AVAILABLE" => Ok(LinkState::Available),
            "RESERVED" => Ok(LinkState::Reserved),
            "SOLD" => Ok(LinkState::Sold),
            "REMOVED" => Ok(LinkState::Removed),
            "UNKNOWN" => Ok(LinkState::Unknown),
            _ => Err(format!("Invalid link state: {}", s)),
        }
    }
}

impl LinkState {
    pub fn from_db(value: &str) -> Self {
        match value {
            "LISTED" => LinkState::Listed,
            "AVAILABLE" => LinkState::Available,
            "RESERVED" => LinkState::Reserved,
            "SOLD" => LinkState::Sold,
            "REMOVED" => LinkState::Removed,
            _ => LinkState::Unknown,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CrawledLinkMetadata {
    pub url: String,
    pub class: LinkClass,
    pub hash: String,
    pub state: LinkState,

    #[serde(
        with = "time::serde::rfc3339::option",
        skip_serializing_if = "Option::is_none",
        default
    )]
    pub last_scraped: Option<OffsetDateTime>,

    #[serde(with = "time::serde::rfc3339")]
    pub created: OffsetDateTime,

    #[serde(with = "time::serde::rfc3339")]
    pub updated: OffsetDateTime,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpiderRunResult {
    pub total_links: usize,
    pub product_urls_count: usize,
    pub product_pattern: Option<String>,
}

#[derive(Debug, Clone)]
pub struct SpiderServiceConfig {
    pub db_batch_size: usize,
    pub max_sample_urls: usize,
}

impl Default for SpiderServiceConfig {
    fn default() -> Self {
        Self {
            db_batch_size: 100,
            max_sample_urls: 500,
        }
    }
}
