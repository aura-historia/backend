use std::fmt::{Display, Formatter};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Scope {
    ProductListingsWrite,
    ShopsRead,
    ShopsWrite,
    PartnerShopApplicationsWrite,
    PartnerShopsRead,
    PartnerShopsWrite,
    UsersRead,
    UsersWrite,
    AccessTokensRead,
    AccessTokensWrite,
    SearchFiltersWrite,
    WatchlistRead,
    WatchlistWrite,
}

impl Scope {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ProductListingsWrite => "product-listings:write",
            Self::ShopsRead => "shops:read",
            Self::ShopsWrite => "shops:write",
            Self::PartnerShopApplicationsWrite => "partner-shop-applications:write",
            Self::PartnerShopsRead => "partner-shops:read",
            Self::PartnerShopsWrite => "partner-shops:write",
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
        assert_eq!(
            "product-listings:write",
            Scope::ProductListingsWrite.as_str()
        );
        assert_eq!("access-tokens:read", Scope::AccessTokensRead.to_string());
    }
}
