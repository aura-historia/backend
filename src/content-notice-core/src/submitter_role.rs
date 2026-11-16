/// Who submits a content notice, as declared by the submitter.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, strum_macros::EnumIter)]
pub enum SubmitterRole {
    /// The provider (operator) of the listing source.
    Provider,
    /// The holder of rights in the listed content, such as a photographer.
    RightsHolder,
    /// Anyone reporting content they consider illegal. They may stay anonymous.
    IllegalContentReporter,
    /// Anyone else, for example a visitor reporting incorrect information.
    Other,
}

impl SubmitterRole {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Provider => "PROVIDER",
            Self::RightsHolder => "RIGHTS_HOLDER",
            Self::IllegalContentReporter => "ILLEGAL_CONTENT_REPORTER",
            Self::Other => "OTHER",
        }
    }

    /// Whether a notice from this role must name the notifier and their email address. A
    /// report of illegal content may be anonymous: DSA Recital 50 allows but does not require
    /// identification, and Art. 16(2)(c) exempts reports of child sexual abuse offences.
    pub fn requires_identification(self) -> bool {
        !matches!(self, Self::IllegalContentReporter)
    }

    pub fn from_code(value: &str) -> Option<Self> {
        use strum::IntoEnumIterator;

        Self::iter().find(|role| role.as_str() == value)
    }
}

#[cfg(test)]
mod tests {
    use super::SubmitterRole;
    use strum::IntoEnumIterator;

    #[test]
    fn should_round_trip_every_persisted_code() {
        for role in SubmitterRole::iter() {
            assert_eq!(Some(role), SubmitterRole::from_code(role.as_str()));
        }
        assert_eq!(
            "ILLEGAL_CONTENT_REPORTER",
            SubmitterRole::IllegalContentReporter.as_str()
        );
        assert_eq!("OTHER", SubmitterRole::Other.as_str());
    }

    #[test]
    fn should_reject_unknown_or_noncanonical_code() {
        assert_eq!(None, SubmitterRole::from_code("provider"));
        assert_eq!(None, SubmitterRole::from_code("OPERATOR"));
    }

    #[test]
    fn should_require_identification_from_every_role_except_illegal_content_reporters() {
        for role in SubmitterRole::iter() {
            assert_eq!(
                role != SubmitterRole::IllegalContentReporter,
                role.requires_identification()
            );
        }
    }
}
