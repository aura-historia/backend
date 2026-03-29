use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum UrlClass {
    Product,
    Category,
    Imprint,
    Info,
    Other,
}

impl std::fmt::Display for UrlClass {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            UrlClass::Product => write!(f, "product"),
            UrlClass::Category => write!(f, "category"),
            UrlClass::Imprint => write!(f, "imprint"),
            UrlClass::Info => write!(f, "info"),
            UrlClass::Other => write!(f, "other"),
        }
    }
}

impl std::str::FromStr for UrlClass {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "product" => Ok(UrlClass::Product),
            "category" => Ok(UrlClass::Category),
            "imprint" => Ok(UrlClass::Imprint),
            "info" => Ok(UrlClass::Info),
            "other" => Ok(UrlClass::Other),
            _ => Err(format!("Invalid URL class: {s}")),
        }
    }
}

impl UrlClass {
    pub fn as_str(self) -> &'static str {
        match self {
            UrlClass::Product => "product",
            UrlClass::Category => "category",
            UrlClass::Imprint => "imprint",
            UrlClass::Info => "info",
            UrlClass::Other => "other",
        }
    }

    pub fn from_db(value: &str) -> Self {
        match value {
            "product" => UrlClass::Product,
            "category" => UrlClass::Category,
            "imprint" => UrlClass::Imprint,
            "info" => UrlClass::Info,
            _ => UrlClass::Other,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum UrlState {
    Listed,
    Available,
    Reserved,
    Sold,
    Removed,
    Unknown,
}

impl std::fmt::Display for UrlState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            UrlState::Listed => write!(f, "LISTED"),
            UrlState::Available => write!(f, "AVAILABLE"),
            UrlState::Reserved => write!(f, "RESERVED"),
            UrlState::Sold => write!(f, "SOLD"),
            UrlState::Removed => write!(f, "REMOVED"),
            UrlState::Unknown => write!(f, "UNKNOWN"),
        }
    }
}

impl std::str::FromStr for UrlState {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "LISTED" => Ok(UrlState::Listed),
            "AVAILABLE" => Ok(UrlState::Available),
            "RESERVED" => Ok(UrlState::Reserved),
            "SOLD" => Ok(UrlState::Sold),
            "REMOVED" => Ok(UrlState::Removed),
            "UNKNOWN" => Ok(UrlState::Unknown),
            _ => Err(format!("Invalid URL state: {s}")),
        }
    }
}

impl UrlState {
    pub fn from_db(value: &str) -> Self {
        match value {
            "LISTED" => UrlState::Listed,
            "AVAILABLE" => UrlState::Available,
            "RESERVED" => UrlState::Reserved,
            "SOLD" => UrlState::Sold,
            "REMOVED" => UrlState::Removed,
            _ => UrlState::Unknown,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CrawledUrlMetadata {
    pub url: String,
    pub class: UrlClass,
    pub hash: String,
    pub state: UrlState,

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
