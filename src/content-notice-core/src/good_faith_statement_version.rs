/// The wording of the good-faith statement a submitter confirmed.
///
/// A notice can only be submitted with a confirmed statement, so there is no "not confirmed"
/// value. Each variant denotes one fixed wording; a changed wording gets a new variant.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, strum_macros::EnumIter)]
pub enum GoodFaithStatementVersion {
    /// "Ich versichere in gutem Glauben, dass meine Angaben richtig und vollständig sind." /
    /// "I confirm in good faith that the information I have given is accurate and complete."
    GoodFaithV1,
}

impl GoodFaithStatementVersion {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::GoodFaithV1 => "GOOD_FAITH_V1",
        }
    }

    pub fn from_code(value: &str) -> Option<Self> {
        use strum::IntoEnumIterator;

        Self::iter().find(|version| version.as_str() == value)
    }
}

#[cfg(test)]
mod tests {
    use super::GoodFaithStatementVersion;

    #[test]
    fn should_use_exact_version_codes() {
        assert_eq!(
            "GOOD_FAITH_V1",
            GoodFaithStatementVersion::GoodFaithV1.as_str()
        );
        assert_eq!(
            Some(GoodFaithStatementVersion::GoodFaithV1),
            GoodFaithStatementVersion::from_code("GOOD_FAITH_V1")
        );
        assert_eq!(None, GoodFaithStatementVersion::from_code("GOOD_FAITH_V2"));
    }
}
