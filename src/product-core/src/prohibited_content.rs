#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(Debug, Clone, Copy, Hash, PartialEq, Eq, Default)]
pub enum ProhibitedContent {
    #[default]
    Unknown,
    None,
    NaziGermany,
}

impl ProhibitedContent {
    pub fn is_safe(&self) -> bool {
        match self {
            ProhibitedContent::Unknown => false,
            ProhibitedContent::None => true,
            ProhibitedContent::NaziGermany => false,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            ProhibitedContent::Unknown => "UNKNOWN",
            ProhibitedContent::None => "NONE",
            ProhibitedContent::NaziGermany => "NAZI_GERMANY",
        }
    }
}

#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ProhibitedContentReason {
    ProductText,
}

impl ProhibitedContentReason {
    pub fn as_str(&self) -> &'static str {
        match self {
            ProhibitedContentReason::ProductText => "PRODUCT_TEXT",
        }
    }
}
