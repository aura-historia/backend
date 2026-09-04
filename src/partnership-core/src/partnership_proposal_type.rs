#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, strum_macros::EnumIter)]
pub enum PartnershipProposalType {
    ExistingListingSource,
    ProposedListingSource,
}

impl PartnershipProposalType {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ExistingListingSource => "EXISTING_LISTING_SOURCE",
            Self::ProposedListingSource => "PROPOSED_LISTING_SOURCE",
        }
    }

    pub fn from_code(value: &str) -> Option<Self> {
        use strum::IntoEnumIterator;

        Self::iter().find(|proposal_type| proposal_type.as_str() == value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;
    use strum::IntoEnumIterator;

    #[test]
    fn should_use_exact_proposal_type_codes() {
        assert_eq!(
            "EXISTING_LISTING_SOURCE",
            PartnershipProposalType::ExistingListingSource.as_str()
        );
        assert_eq!(
            "PROPOSED_LISTING_SOURCE",
            PartnershipProposalType::ProposedListingSource.as_str()
        );
    }

    #[test]
    fn should_parse_each_exact_proposal_type_code() {
        for proposal_type in PartnershipProposalType::iter() {
            assert_eq!(
                Some(proposal_type),
                PartnershipProposalType::from_code(proposal_type.as_str())
            );
        }

        assert_eq!(
            None,
            PartnershipProposalType::from_code("existing_listing_source")
        );
        assert_eq!(
            None,
            PartnershipProposalType::from_code("UNSUPPORTED_PROPOSAL_TYPE")
        );
    }

    #[test]
    fn should_use_unique_proposal_type_codes() {
        let proposal_types = PartnershipProposalType::iter().collect::<Vec<_>>();

        assert_eq!(
            proposal_types.len(),
            proposal_types
                .iter()
                .map(|proposal_type| proposal_type.as_str())
                .collect::<HashSet<_>>()
                .len()
        );
    }
}
