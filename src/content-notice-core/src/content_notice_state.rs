/// Lifecycle state of a content notice. Decision states are added with the decision workflow.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, strum_macros::EnumIter)]
pub enum ContentNoticeState {
    Submitted,
}

impl ContentNoticeState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Submitted => "SUBMITTED",
        }
    }

    pub fn from_code(value: &str) -> Option<Self> {
        use strum::IntoEnumIterator;

        Self::iter().find(|state| state.as_str() == value)
    }
}

#[cfg(test)]
mod tests {
    use super::ContentNoticeState;

    #[test]
    fn should_use_exact_state_codes() {
        assert_eq!("SUBMITTED", ContentNoticeState::Submitted.as_str());
        assert_eq!(
            Some(ContentNoticeState::Submitted),
            ContentNoticeState::from_code("SUBMITTED")
        );
        assert_eq!(None, ContentNoticeState::from_code("submitted"));
        assert_eq!(None, ContentNoticeState::from_code("DECIDED"));
    }
}
