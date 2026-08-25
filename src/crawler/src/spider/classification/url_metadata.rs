use product_listing_core::product_state::ProductState;
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

impl From<ProductState> for UrlState {
    fn from(value: ProductState) -> Self {
        match value {
            ProductState::Listed => UrlState::Listed,
            ProductState::Available => UrlState::Available,
            ProductState::Reserved => UrlState::Reserved,
            ProductState::Sold => UrlState::Sold,
            ProductState::Removed => UrlState::Removed,
            ProductState::Unknown => UrlState::Unknown,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CrawledUrlMetadata {
    pub url: String,
    pub class: UrlClass,
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

#[cfg(test)]
mod tests {
    use super::UrlState;
    use product_listing_core::product_state::ProductState;

    #[test]
    fn should_map_product_states_to_url_states() {
        let cases = [
            (ProductState::Listed, UrlState::Listed),
            (ProductState::Available, UrlState::Available),
            (ProductState::Reserved, UrlState::Reserved),
            (ProductState::Sold, UrlState::Sold),
            (ProductState::Removed, UrlState::Removed),
            (ProductState::Unknown, UrlState::Unknown),
        ];

        for (product_state, expected_url_state) in cases {
            assert_eq!(UrlState::from(product_state), expected_url_state);
        }
    }
}
