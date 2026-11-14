/// Why a content notice was submitted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, strum_macros::EnumIter)]
pub enum ReasonCategory {
    /// The provider asks to stop being listed. Only a provider may choose it.
    ProviderOptOut,
    RightsInfringement,
    IllegalContent,
    IncorrectInformation,
    Other,
}

impl ReasonCategory {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ProviderOptOut => "PROVIDER_OPT_OUT",
            Self::RightsInfringement => "RIGHTS_INFRINGEMENT",
            Self::IllegalContent => "ILLEGAL_CONTENT",
            Self::IncorrectInformation => "INCORRECT_INFORMATION",
            Self::Other => "OTHER",
        }
    }

    pub fn from_code(value: &str) -> Option<Self> {
        use strum::IntoEnumIterator;

        Self::iter().find(|category| category.as_str() == value)
    }

    /// A plain provider opt-out is self-explanatory; every other category needs a reasoned
    /// explanation.
    pub fn requires_explanation(self) -> bool {
        !matches!(self, Self::ProviderOptOut)
    }
}

#[cfg(test)]
mod tests {
    use super::ReasonCategory;
    use strum::IntoEnumIterator;

    #[test]
    fn should_round_trip_every_persisted_code() {
        for category in ReasonCategory::iter() {
            assert_eq!(Some(category), ReasonCategory::from_code(category.as_str()));
        }
        assert_eq!(
            "INCORRECT_INFORMATION",
            ReasonCategory::IncorrectInformation.as_str()
        );
    }

    #[test]
    fn should_reject_unknown_or_noncanonical_code() {
        assert_eq!(None, ReasonCategory::from_code("provider_opt_out"));
        assert_eq!(None, ReasonCategory::from_code("SPAM"));
    }

    #[test]
    fn should_require_explanation_for_every_category_except_provider_opt_out() {
        for category in ReasonCategory::iter() {
            assert_eq!(
                category != ReasonCategory::ProviderOptOut,
                category.requires_explanation()
            );
        }
    }
}
