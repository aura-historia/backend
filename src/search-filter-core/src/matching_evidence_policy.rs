/// Evidence capabilities resolved by the saved-filter write use case.
/// Execution consumes these capabilities without consulting subscriptions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MatchingEvidencePolicy {
    TextOnly,
    TextAndOneImage,
    EnhancedTextAndFourImages,
}

impl MatchingEvidencePolicy {
    pub const VERSION: &'static str = "matching-evidence-v1";

    pub const fn image_limit(self) -> usize {
        match self {
            Self::TextOnly => 0,
            Self::TextAndOneImage => 1,
            Self::EnhancedTextAndFourImages => 4,
        }
    }

    pub const fn permits_enhanced_intent(self) -> bool {
        matches!(self, Self::EnhancedTextAndFourImages)
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::TextOnly => "TEXT_ONLY",
            Self::TextAndOneImage => "TEXT_AND_ONE_IMAGE",
            Self::EnhancedTextAndFourImages => "ENHANCED_TEXT_AND_FOUR_IMAGES",
        }
    }

    pub fn from_code(value: &str) -> Option<Self> {
        match value {
            "TEXT_ONLY" => Some(Self::TextOnly),
            "TEXT_AND_ONE_IMAGE" => Some(Self::TextAndOneImage),
            "ENHANCED_TEXT_AND_FOUR_IMAGES" => Some(Self::EnhancedTextAndFourImages),
            _ => None,
        }
    }
}
