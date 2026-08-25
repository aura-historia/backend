use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum UrlClass {
    ProductListing,
    Category,
    Imprint,
    Info,
    Other,
}

impl std::fmt::Display for UrlClass {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            UrlClass::ProductListing => write!(f, "product"),
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
            "product" => Ok(UrlClass::ProductListing),
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
            UrlClass::ProductListing => "product",
            UrlClass::Category => "category",
            UrlClass::Imprint => "imprint",
            UrlClass::Info => "info",
            UrlClass::Other => "other",
        }
    }

    pub fn from_db(value: &str) -> Self {
        match value {
            "product" => UrlClass::ProductListing,
            "category" => UrlClass::Category,
            "imprint" => UrlClass::Imprint,
            "info" => UrlClass::Info,
            _ => UrlClass::Other,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum UrlPresence {
    Present,
    Withdrawn,
}

impl std::fmt::Display for UrlPresence {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Present => "PRESENT",
            Self::Withdrawn => "WITHDRAWN",
        })
    }
}

impl std::str::FromStr for UrlPresence {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "PRESENT" => Ok(Self::Present),
            "WITHDRAWN" => Ok(Self::Withdrawn),
            _ => Err(format!("Invalid URL presence: {value}")),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CrawledUrlMetadata {
    pub url: String,
    pub class: UrlClass,
    pub presence: UrlPresence,
    /// Canonical crawler-local availability code, if the last scrape asserted one.
    pub availability: Option<String>,

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

#[cfg(test)]
mod tests {
    use super::UrlPresence;

    #[test]
    fn should_parse_only_presence_values() {
        assert_eq!("PRESENT".parse(), Ok(UrlPresence::Present));
        assert_eq!("WITHDRAWN".parse(), Ok(UrlPresence::Withdrawn));
        assert!("AVAILABLE".parse::<UrlPresence>().is_err());
    }
}
