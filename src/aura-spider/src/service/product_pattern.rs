use regex::Regex;

#[derive(Debug, Clone)]
pub enum ProductPattern {
    Known(Regex),
    Unknown,
}

impl ProductPattern {
    pub fn as_regex(&self) -> Option<&Regex> {
        match self {
            ProductPattern::Known(regex) => Some(regex),
            ProductPattern::Unknown => None,
        }
    }

    pub fn is_known(&self) -> bool {
        matches!(self, ProductPattern::Known(_))
    }

    pub fn is_unknown(&self) -> bool {
        matches!(self, ProductPattern::Unknown)
    }
}

impl From<Regex> for ProductPattern {
    fn from(value: Regex) -> Self {
        ProductPattern::Known(value)
    }
}
