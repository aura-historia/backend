use crate::spider::classification::product_url_pattern::ProductUrlPattern;
use crate::spider::classification::url_pattern_service::DecidedUrlPattern;

#[derive(Debug, Clone)]
pub enum ProductListingPattern {
    Known(ProductUrlPattern),
    Unknown,
}

impl ProductListingPattern {
    pub fn as_pattern(&self) -> Option<&ProductUrlPattern> {
        match self {
            ProductListingPattern::Known(regex) => Some(regex),
            ProductListingPattern::Unknown => None,
        }
    }

    pub fn is_known(&self) -> bool {
        matches!(self, ProductListingPattern::Known(_))
    }

    pub fn is_unknown(&self) -> bool {
        matches!(self, ProductListingPattern::Unknown)
    }
}

impl From<ProductUrlPattern> for ProductListingPattern {
    fn from(value: ProductUrlPattern) -> Self {
        ProductListingPattern::Known(value)
    }
}

impl From<DecidedUrlPattern> for ProductListingPattern {
    fn from(decision: DecidedUrlPattern) -> Self {
        match decision {
            DecidedUrlPattern::Pattern(pattern) => ProductListingPattern::Known(pattern),
            DecidedUrlPattern::NoPattern => ProductListingPattern::Unknown,
        }
    }
}
