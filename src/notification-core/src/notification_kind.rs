#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, strum_macros::EnumIter)]
pub enum NotificationKind {
    WatchlistPriceChanged,
    WatchlistAvailabilityChanged,
    SearchFilterMatch,
    PartnerApplicationApproved,
    PartnerApplicationRejected,
    PartnershipApplicationApproved,
    PartnershipApplicationRejected,
}

impl NotificationKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::WatchlistPriceChanged => "WATCHLIST_PRICE_CHANGED",
            Self::WatchlistAvailabilityChanged => "WATCHLIST_AVAILABILITY_CHANGED",
            Self::SearchFilterMatch => "SEARCH_FILTER_MATCH",
            Self::PartnerApplicationApproved => "PARTNER_APPLICATION_APPROVED",
            Self::PartnerApplicationRejected => "PARTNER_APPLICATION_REJECTED",
            Self::PartnershipApplicationApproved => "PARTNERSHIP_APPLICATION_APPROVED",
            Self::PartnershipApplicationRejected => "PARTNERSHIP_APPLICATION_REJECTED",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;
    use strum::IntoEnumIterator;

    #[test]
    fn should_define_unique_canonical_kind_identifiers() {
        let kinds = NotificationKind::iter().collect::<Vec<_>>();
        let identifiers = kinds
            .iter()
            .map(|kind| kind.as_str())
            .collect::<HashSet<_>>();

        assert_eq!(kinds.len(), identifiers.len());
        assert_eq!(
            vec![
                "WATCHLIST_PRICE_CHANGED",
                "WATCHLIST_AVAILABILITY_CHANGED",
                "SEARCH_FILTER_MATCH",
                "PARTNER_APPLICATION_APPROVED",
                "PARTNER_APPLICATION_REJECTED",
                "PARTNERSHIP_APPLICATION_APPROVED",
                "PARTNERSHIP_APPLICATION_REJECTED",
            ],
            kinds
                .into_iter()
                .map(NotificationKind::as_str)
                .collect::<Vec<_>>()
        );
    }
}
