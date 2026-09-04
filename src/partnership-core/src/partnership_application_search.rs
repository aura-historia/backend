use crate::{
    partnership_application_state::PartnershipApplicationState,
    partnership_proposal_type::PartnershipProposalType,
};
use domain_primitives::query::{any_of_query::AnyOfQuery, range_query::RangeQuery};
use listing_source_core::ListingSourceId;
use time::OffsetDateTime;
use user_core::user_id::UserId;

#[derive(Debug, Clone, PartialEq, Default)]
pub struct PartnershipApplicationSearch {
    pub state_query: AnyOfQuery<PartnershipApplicationState>,
    pub applicant_user_id: Option<UserId>,
    pub proposal_type_query: AnyOfQuery<PartnershipProposalType>,
    pub listing_source_id: Option<ListingSourceId>,
    pub created: Option<RangeQuery<OffsetDateTime>>,
    pub updated: Option<RangeQuery<OffsetDateTime>>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_default_partnership_application_search_to_empty_filters() {
        let search = PartnershipApplicationSearch::default();

        assert!(search.state_query.is_empty());
        assert_eq!(None, search.applicant_user_id);
        assert!(search.proposal_type_query.is_empty());
        assert_eq!(None, search.listing_source_id);
        assert_eq!(None, search.created);
        assert_eq!(None, search.updated);
    }
}
