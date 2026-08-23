use serde::{Deserialize, Serialize};

#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(Debug, Clone, Copy, Hash, PartialEq, Eq, Default, Serialize, Deserialize)]
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

#[cfg(test)]
mod tests {
    use super::*;

    #[rstest::rstest]
    #[case(ProhibitedContent::Unknown, false, "UNKNOWN")]
    #[case(ProhibitedContent::None, true, "NONE")]
    #[case(ProhibitedContent::NaziGermany, false, "NAZI_GERMANY")]
    fn should_report_safety_and_string_for_prohibited_content(
        #[case] content: ProhibitedContent,
        #[case] safe: bool,
        #[case] value: &'static str,
    ) {
        assert_eq!(safe, content.is_safe());
        assert_eq!(value, content.as_str());
    }

    #[test]
    fn should_report_reason_string() {
        assert_eq!(
            "PRODUCT_TEXT",
            ProhibitedContentReason::ProductText.as_str()
        );
    }
}
