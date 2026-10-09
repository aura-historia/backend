use std::fmt::{Display, Formatter};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Scope {
    AuctionsRead,
    AuctionsWrite,
    ProductListingsWrite,
    ListingSourcesRead,
    ListingSourcesWrite,
    PartiesRead,
    PartiesWrite,
    PartnershipApplicationsRead,
    PartnershipApplicationsWrite,
    PartnershipsRead,
    PartnershipsWrite,
    AdminOverviewRead,
    UsersRead,
    UsersWrite,
    AccessTokensRead,
    AccessTokensWrite,
    SearchFiltersRead,
    SearchFiltersWrite,
    NotificationsRead,
    NotificationsWrite,
    WatchlistRead,
    WatchlistWrite,
}

impl Scope {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::AuctionsRead => "auctions:read",
            Self::AuctionsWrite => "auctions:write",
            Self::ListingSourcesRead => "listing-sources:read",
            Self::PartiesRead => "parties:read",
            Self::PartiesWrite => "parties:write",
            Self::PartnershipApplicationsRead => "partnership-applications:read",
            Self::PartnershipApplicationsWrite => "partnership-applications:write",
            Self::PartnershipsRead => "partnerships:read",
            Self::PartnershipsWrite => "partnerships:write",
            Self::AdminOverviewRead => "admin-overview:read",
            Self::SearchFiltersRead => "search-filters:read",
            Self::NotificationsRead => "notifications:read",
            Self::NotificationsWrite => "notifications:write",
            Self::ProductListingsWrite => "product-listings:write",
            Self::ListingSourcesWrite => "listing-sources:write",
            Self::UsersRead => "users:read",
            Self::UsersWrite => "users:write",
            Self::AccessTokensRead => "access-tokens:read",
            Self::AccessTokensWrite => "access-tokens:write",
            Self::SearchFiltersWrite => "search-filters:write",
            Self::WatchlistRead => "watchlist:read",
            Self::WatchlistWrite => "watchlist:write",
        }
    }

    pub fn as_scope_str(self) -> &'static str {
        self.as_str()
    }
}

impl Display for Scope {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::Scope;

    #[test]
    fn should_preserve_oauth_scope_strings() {
        assert_eq!("auctions:read", Scope::AuctionsRead.as_str());
        assert_eq!("auctions:read", Scope::AuctionsRead.to_string());
        assert_eq!(
            "product-listings:write",
            Scope::ProductListingsWrite.as_str()
        );
        assert_eq!(
            "listing-sources:write",
            Scope::ListingSourcesWrite.to_string()
        );
        assert_eq!("access-tokens:read", Scope::AccessTokensRead.to_string());
    }
}
