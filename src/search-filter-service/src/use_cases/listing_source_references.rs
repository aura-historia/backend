//! Public cards of the ListingSources a saved search includes or excludes, so a client can name
//! them. The listing-source context owns which sources are public; a referenced ID that no longer
//! resolves is left out, while a failed lookup fails the caller.
use crate::ports::SearchFilterView;
use application::operation_context::OperationContext;
use listing_source_core::ListingSourceId;
use listing_source_service::use_cases::queries::get_public_listing_sources_by_ids::{
    GetPublicListingSourcesByIdsError, GetPublicListingSourcesByIdsRequest,
    GetPublicListingSourcesByIdsUseCase,
};
use listing_source_service::use_cases::queries::public_listing_source::PublicListingSourceSummary;
use product_listing_core::product_listing_search::ProductListingSearch;
use std::collections::{BTreeMap, BTreeSet};

/// A saved search filter with the public ListingSources its search references.
#[derive(Debug, Clone, PartialEq)]
pub struct SearchFilterDetails {
    pub view: SearchFilterView,
    /// One card per included or excluded ListingSource that still resolves, ordered by ID.
    pub listing_sources: Vec<PublicListingSourceSummary>,
}

/// The ListingSources of several searches, resolved in one lookup.
pub(crate) struct ReferencedListingSources {
    requested: BTreeSet<ListingSourceId>,
    found: BTreeMap<ListingSourceId, PublicListingSourceSummary>,
}

impl ReferencedListingSources {
    pub(crate) async fn resolve<'a>(
        lookup: &dyn GetPublicListingSourcesByIdsUseCase,
        context: &OperationContext,
        searches: impl IntoIterator<Item = &'a ProductListingSearch>,
    ) -> Result<Self, GetPublicListingSourcesByIdsError> {
        let requested = searches
            .into_iter()
            .flat_map(referenced_ids)
            .collect::<BTreeSet<_>>();
        let found = lookup
            .execute(
                context,
                GetPublicListingSourcesByIdsRequest {
                    ids: requested.clone(),
                },
            )
            .await?
            .into_iter()
            .map(|summary| (summary.listing_source_id, summary))
            .collect();
        Ok(Self { requested, found })
    }

    /// Whether every ListingSource `search` references was part of the lookup.
    pub(crate) fn covers(&self, search: &ProductListingSearch) -> bool {
        referenced_ids(search).all(|id| self.requested.contains(&id))
    }

    pub(crate) fn details(&self, view: SearchFilterView) -> SearchFilterDetails {
        let listing_sources = referenced_ids(&view.search)
            .collect::<BTreeSet<_>>()
            .into_iter()
            .filter_map(|id| self.found.get(&id).cloned())
            .collect();
        SearchFilterDetails {
            view,
            listing_sources,
        }
    }
}

fn referenced_ids(search: &ProductListingSearch) -> impl Iterator<Item = ListingSourceId> + '_ {
    search
        .listing_source_id_query
        .iter()
        .chain(search.exclude_listing_source_id_query.iter())
        .copied()
}

#[cfg(test)]
mod tests {
    use super::*;
    use application::error::static_error;
    use application::operation_context::{CorrelationId, Principal, RequestId};
    use domain_primitives::query::any_of_query::AnyOfQuery;
    use listing_source_core::{ListingSourceName, ListingSourceSlugId};
    use listing_source_service::use_cases::queries::public_listing_source::PublicListingSourceOperatorSummary;
    use localization::Language;
    use money::Currency;
    use search_filter_core::search_filter_state::SearchFilterState;
    use search_filter_core::user_search_filter_id::UserSearchFilterId;
    use std::sync::{Arc, Mutex};
    use time::OffsetDateTime;
    use user_core::user_id::UserId;

    /// Answers from `stored`, records every request and fails when `fails` is set.
    #[derive(Default)]
    struct FakeListingSources {
        stored: Vec<PublicListingSourceSummary>,
        fails: bool,
        requests: Arc<Mutex<Vec<BTreeSet<ListingSourceId>>>>,
    }

    #[async_trait::async_trait]
    impl GetPublicListingSourcesByIdsUseCase for FakeListingSources {
        async fn execute(
            &self,
            _context: &OperationContext,
            request: GetPublicListingSourcesByIdsRequest,
        ) -> Result<Vec<PublicListingSourceSummary>, GetPublicListingSourcesByIdsError> {
            self.requests.lock().unwrap().push(request.ids.clone());
            if self.fails {
                return Err(GetPublicListingSourcesByIdsError::TemporarilyUnavailable {
                    source: static_error("listing sources unavailable"),
                });
            }
            Ok(self
                .stored
                .iter()
                .filter(|summary| request.ids.contains(&summary.listing_source_id))
                .cloned()
                .collect())
        }
    }

    fn summary(id: ListingSourceId) -> PublicListingSourceSummary {
        PublicListingSourceSummary {
            listing_source_id: id,
            listing_source_slug_id: ListingSourceSlugId::raw(format!("source-{}", id.as_uuid()))
                .unwrap(),
            name: ListingSourceName::try_from("Source").unwrap(),
            operator: PublicListingSourceOperatorSummary {
                name: "Operator".try_into().unwrap(),
            },
            website: None,
            image: None,
        }
    }

    fn context() -> OperationContext {
        OperationContext {
            principal: Principal::Anonymous,
            request_id: RequestId::new("request"),
            correlation_id: CorrelationId::new("correlation"),
        }
    }

    fn search(include: &[ListingSourceId], exclude: &[ListingSourceId]) -> ProductListingSearch {
        ProductListingSearch::new(Language::En, Currency::Eur)
            .with_listing_source_id_query(AnyOfQuery::from_iter(include.iter().copied()))
            .with_exclude_listing_source_id_query(AnyOfQuery::from_iter(exclude.iter().copied()))
    }

    fn view(search: ProductListingSearch) -> SearchFilterView {
        SearchFilterView {
            search_filter_id: UserSearchFilterId::new(),
            user_id: UserId::new(),
            name: "Saved".into(),
            notifications: true,
            state: SearchFilterState::Active,
            search,
            embedding: None,
            created: OffsetDateTime::UNIX_EPOCH,
            updated: OffsetDateTime::UNIX_EPOCH,
        }
    }

    #[tokio::test]
    async fn should_resolve_included_and_excluded_sources_of_all_searches_in_one_lookup() {
        let [included, excluded, shared, deleted] = [(); 4].map(|()| ListingSourceId::new());
        let lookup = FakeListingSources {
            stored: vec![summary(included), summary(excluded), summary(shared)],
            ..Default::default()
        };
        let first = view(search(&[included, shared], &[excluded]));
        let second = view(search(&[deleted], &[shared]));

        let references =
            ReferencedListingSources::resolve(&lookup, &context(), [&first.search, &second.search])
                .await
                .unwrap();
        let first = references.details(first);
        let second = references.details(second);

        assert_eq!(
            vec![BTreeSet::from([included, excluded, shared, deleted])],
            *lookup.requests.lock().unwrap()
        );
        let mut expected_first = [included, excluded, shared];
        expected_first.sort();
        assert_eq!(
            expected_first.to_vec(),
            ids_of(&first),
            "every resolvable reference once, ordered by ID"
        );
        assert_eq!(vec![shared], ids_of(&second), "unresolved IDs are omitted");
        assert!(references.covers(&first.view.search));
        assert!(!references.covers(&search(&[ListingSourceId::new()], &[])));
    }

    #[tokio::test]
    async fn should_list_a_source_once_when_a_search_includes_and_excludes_it() {
        let id = ListingSourceId::new();
        let lookup = FakeListingSources {
            stored: vec![summary(id)],
            ..Default::default()
        };
        let filter = view(search(&[id], &[id]));

        let references = ReferencedListingSources::resolve(&lookup, &context(), [&filter.search])
            .await
            .unwrap();

        assert_eq!(vec![id], ids_of(&references.details(filter)));
    }

    fn ids_of(details: &SearchFilterDetails) -> Vec<ListingSourceId> {
        details
            .listing_sources
            .iter()
            .map(|summary| summary.listing_source_id)
            .collect()
    }

    #[tokio::test]
    async fn should_fail_instead_of_omitting_sources_when_the_lookup_fails() {
        let lookup = FakeListingSources {
            fails: true,
            ..Default::default()
        };
        let filter = view(search(&[ListingSourceId::new()], &[]));

        let result = ReferencedListingSources::resolve(&lookup, &context(), [&filter.search]).await;

        assert!(matches!(
            result,
            Err(GetPublicListingSourcesByIdsError::TemporarilyUnavailable { .. })
        ));
    }
}
