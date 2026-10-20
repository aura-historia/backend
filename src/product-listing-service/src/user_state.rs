use application::operation_context::{CredentialCapability, Principal};
use application::personalized::Personalized;
use notification_core::notification_id::NotificationId;
use search_filter_core::{
    user_search_filter_id::UserSearchFilterId, user_search_filter_name::UserSearchFilterName,
};

/// Complete per-user state loaded for one listing.
///
/// Use cases enforce hidden-item, entitlement and content policy from this state. They return
/// only the sections permitted by [`ProductListingUserStateDisclosure`].
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ProductListingUserState {
    pub shipping: Option<crate::shipping::PersonalizedShipping>,
    pub watchlist: WatchlistUserState,
    pub content_visibility: ContentVisibilityUserState,
    pub notification: NotificationUserState,
    pub search_filter: SearchFilterUserState,
}

/// Per-user listing state disclosed to the caller. Sections it may not read are absent.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ProductListingUserStateView {
    pub shipping: Option<crate::shipping::PersonalizedShipping>,
    pub watchlist: Option<WatchlistUserState>,
    pub content_visibility: Option<ContentVisibilityUserState>,
    pub notification: Option<NotificationUserState>,
    pub search_filter: Option<SearchFilterUserState>,
}

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct WatchlistUserState {
    pub watching: bool,
    pub notifications: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct ContentVisibilityUserState {
    pub show_unassessed_or_sensitive_content: bool,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct NotificationUserState {
    pub unseen_notification_ids: Vec<NotificationId>,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct SearchFilterUserState {
    pub matched: bool,
    pub hidden: bool,
    pub user_search_filter_id: Option<UserSearchFilterId>,
    pub user_search_filter_name: Option<UserSearchFilterName>,
    pub match_feedback: Option<bool>,
    pub shipping_match_valuation: Option<search_filter_core::ShippingMatchValuation>,
}

/// Private user-state sections a principal may receive with listing data.
///
/// Delegated credentials are closed-world: each section requires its own read scope, whether or
/// not the listing endpoint itself is public. Cognito users, services and the system receive
/// every section.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProductListingUserStateDisclosure {
    watchlist: bool,
    notification: bool,
    search_filter: bool,
    account: bool,
}

impl ProductListingUserStateDisclosure {
    pub fn for_principal(principal: &Principal) -> Self {
        let may_read = |capability| principal.require_credential_capability(capability).is_ok();
        Self {
            watchlist: may_read(CredentialCapability::WatchlistRead),
            notification: may_read(CredentialCapability::NotificationsRead),
            search_filter: may_read(CredentialCapability::SearchFiltersRead),
            account: may_read(CredentialCapability::UsersRead),
        }
    }

    pub fn disclose<Item>(
        self,
        personalized: Personalized<Item, ProductListingUserState>,
    ) -> Personalized<Item, ProductListingUserStateView> {
        Personalized {
            item: personalized.item,
            user_state: personalized
                .user_state
                .and_then(|state| self.disclose_state(state)),
        }
    }

    /// Returns `None` when no section may be disclosed, so the response matches public data.
    pub fn disclose_state(
        self,
        state: ProductListingUserState,
    ) -> Option<ProductListingUserStateView> {
        let view = ProductListingUserStateView {
            // The browsing destination and content preference are account settings.
            shipping: state.shipping.filter(|_| self.account),
            watchlist: self.watchlist.then_some(state.watchlist),
            content_visibility: self.account.then_some(state.content_visibility),
            notification: self.notification.then_some(state.notification),
            search_filter: self.search_filter.then_some(state.search_filter),
        };
        (view != ProductListingUserStateView::default()).then_some(view)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;
    use user_core::user_id::UserId;

    #[test]
    fn should_default_notification_user_state_seen_to_true() {
        let state = NotificationUserState::default();
        assert!(state.unseen_notification_ids.is_empty());
    }

    #[test]
    fn should_default_product_user_state_notification_seen_to_true() {
        let state = ProductListingUserState::default();
        assert!(state.notification.unseen_notification_ids.is_empty());
    }

    #[test]
    fn should_default_search_filter_user_state_to_not_matched() {
        let state = SearchFilterUserState::default();
        assert!(!state.matched);
        assert!(!state.hidden);
        assert!(state.user_search_filter_id.is_none());
        assert!(state.user_search_filter_name.is_none());
    }

    #[test]
    fn should_default_product_user_state_search_filter_to_not_matched() {
        let state = ProductListingUserState::default();
        assert!(!state.search_filter.matched);
        assert!(!state.search_filter.hidden);
        assert!(state.search_filter.user_search_filter_id.is_none());
        assert!(state.search_filter.user_search_filter_name.is_none());
    }

    fn delegated(capabilities: &[CredentialCapability]) -> Principal {
        Principal::DelegatedUser {
            user_id: UserId::new(),
            capabilities: capabilities.iter().copied().collect::<BTreeSet<_>>(),
        }
    }

    fn personalized_shipping() -> crate::shipping::PersonalizedShipping {
        use shipping_core::assessment::{
            CollectionAssessment, PredicateResult, ShippingAssessment,
        };

        crate::shipping::PersonalizedShipping {
            assessment: ShippingAssessment {
                profile_basis: None,
                eligibility: None,
                selected_zone: None,
                services: Vec::new(),
                reasons: Vec::new(),
                collection: CollectionAssessment {
                    policy: None,
                    applicability: PredicateResult::Undetermined(Vec::new()),
                    eligibility: None,
                    reasons: Vec::new(),
                },
            },
            destination: geo::GeographicDescription::new(
                None,
                Some(geo::CountryCode::DEU),
                None,
                None,
            )
            .ok()
            .flatten()
            .unwrap_or_else(|| panic!("valid test destination")),
            attribution: None,
            evaluated_at: time::OffsetDateTime::UNIX_EPOCH,
        }
    }

    fn full_state() -> ProductListingUserState {
        ProductListingUserState {
            shipping: Some(personalized_shipping()),
            watchlist: WatchlistUserState {
                watching: true,
                notifications: true,
            },
            content_visibility: ContentVisibilityUserState {
                show_unassessed_or_sensitive_content: true,
            },
            notification: NotificationUserState {
                unseen_notification_ids: vec![NotificationId::new()],
            },
            search_filter: SearchFilterUserState {
                matched: true,
                hidden: true,
                user_search_filter_id: Some(UserSearchFilterId::new()),
                user_search_filter_name: Some(UserSearchFilterName::from("Cabinets".to_owned())),
                match_feedback: Some(true),
                shipping_match_valuation: None,
            },
        }
    }

    fn full_view(state: ProductListingUserState) -> ProductListingUserStateView {
        ProductListingUserStateView {
            shipping: state.shipping,
            watchlist: Some(state.watchlist),
            content_visibility: Some(state.content_visibility),
            notification: Some(state.notification),
            search_filter: Some(state.search_filter),
        }
    }

    #[test]
    fn should_disclose_nothing_to_a_delegated_user_without_read_scopes() {
        let disclosure = ProductListingUserStateDisclosure::for_principal(&delegated(&[
            CredentialCapability::ProductListingsRead,
            CredentialCapability::WatchlistWrite,
            CredentialCapability::NotificationsWrite,
            CredentialCapability::SearchFiltersWrite,
            CredentialCapability::UsersWrite,
        ]));

        assert_eq!(None, disclosure.disclose_state(full_state()));
        assert_eq!(
            Personalized {
                item: "listing",
                user_state: None,
            },
            disclosure.disclose(Personalized {
                item: "listing",
                user_state: Some(full_state()),
            })
        );
    }

    #[test]
    fn should_disclose_only_the_section_of_each_read_scope() {
        let state = full_state();
        let cases = [
            (
                CredentialCapability::WatchlistRead,
                ProductListingUserStateView {
                    watchlist: Some(state.watchlist),
                    ..Default::default()
                },
            ),
            (
                CredentialCapability::NotificationsRead,
                ProductListingUserStateView {
                    notification: Some(state.notification.clone()),
                    ..Default::default()
                },
            ),
            (
                CredentialCapability::SearchFiltersRead,
                ProductListingUserStateView {
                    search_filter: Some(state.search_filter.clone()),
                    ..Default::default()
                },
            ),
            (
                CredentialCapability::UsersRead,
                ProductListingUserStateView {
                    shipping: state.shipping.clone(),
                    content_visibility: Some(state.content_visibility),
                    ..Default::default()
                },
            ),
        ];

        for (capability, expected) in cases {
            let disclosure =
                ProductListingUserStateDisclosure::for_principal(&delegated(&[capability]));

            assert_eq!(
                Some(expected),
                disclosure.disclose_state(state.clone()),
                "{capability}"
            );
        }
    }

    #[test]
    fn should_combine_sections_of_multiple_read_scopes() {
        let state = full_state();
        let disclosure = ProductListingUserStateDisclosure::for_principal(&delegated(&[
            CredentialCapability::WatchlistRead,
            CredentialCapability::SearchFiltersRead,
        ]));

        assert_eq!(
            Some(ProductListingUserStateView {
                watchlist: Some(state.watchlist),
                search_filter: Some(state.search_filter.clone()),
                ..Default::default()
            }),
            disclosure.disclose_state(state)
        );
    }

    #[test]
    fn should_disclose_every_section_to_all_read_scopes_and_trusted_principals() {
        let all_read_scopes = delegated(&[
            CredentialCapability::WatchlistRead,
            CredentialCapability::NotificationsRead,
            CredentialCapability::SearchFiltersRead,
            CredentialCapability::UsersRead,
        ]);

        let state = full_state();
        for principal in [
            all_read_scopes,
            Principal::User(UserId::new()),
            Principal::Service("service".to_owned()),
            Principal::System,
        ] {
            assert_eq!(
                Some(full_view(state.clone())),
                ProductListingUserStateDisclosure::for_principal(&principal)
                    .disclose_state(state.clone()),
                "{}",
                principal.kind()
            );
        }
    }

    #[test]
    fn should_disclose_nothing_to_an_anonymous_principal() {
        assert_eq!(
            None,
            ProductListingUserStateDisclosure::for_principal(&Principal::Anonymous)
                .disclose_state(full_state())
        );
    }

    #[test]
    fn should_keep_users_read_gate_when_no_destination_was_resolved() {
        let state = ProductListingUserState {
            shipping: None,
            ..full_state()
        };

        assert_eq!(
            Some(ProductListingUserStateView {
                content_visibility: Some(state.content_visibility),
                ..Default::default()
            }),
            ProductListingUserStateDisclosure::for_principal(&delegated(&[
                CredentialCapability::UsersRead
            ]))
            .disclose_state(state)
        );
    }
}
