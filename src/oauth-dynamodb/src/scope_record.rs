use serde::{Deserialize, Serialize};
use user_core::access_token::Scope;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ScopeRecord {
    ProductsWrite,
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

impl From<Scope> for ScopeRecord {
    fn from(value: Scope) -> Self {
        match value {
            Scope::ProductsWrite => ScopeRecord::ProductsWrite,
            Scope::ShopsRead => ScopeRecord::ShopsRead,
            Scope::ShopsWrite => ScopeRecord::ShopsWrite,
            Scope::PartnerShopApplicationsWrite => ScopeRecord::PartnerShopApplicationsWrite,
            Scope::PartnerShopsRead => ScopeRecord::PartnerShopsRead,
            Scope::PartnerShopsWrite => ScopeRecord::PartnerShopsWrite,
            Scope::UsersRead => ScopeRecord::UsersRead,
            Scope::UsersWrite => ScopeRecord::UsersWrite,
            Scope::AccessTokensRead => ScopeRecord::AccessTokensRead,
            Scope::AccessTokensWrite => ScopeRecord::AccessTokensWrite,
            Scope::SearchFiltersWrite => ScopeRecord::SearchFiltersWrite,
            Scope::WatchlistRead => ScopeRecord::WatchlistRead,
            Scope::WatchlistWrite => ScopeRecord::WatchlistWrite,
        }
    }
}

impl From<ScopeRecord> for Scope {
    fn from(value: ScopeRecord) -> Self {
        match value {
            ScopeRecord::ProductsWrite => Scope::ProductsWrite,
            ScopeRecord::ShopsRead => Scope::ShopsRead,
            ScopeRecord::ShopsWrite => Scope::ShopsWrite,
            ScopeRecord::PartnerShopApplicationsWrite => Scope::PartnerShopApplicationsWrite,
            ScopeRecord::PartnerShopsRead => Scope::PartnerShopsRead,
            ScopeRecord::PartnerShopsWrite => Scope::PartnerShopsWrite,
            ScopeRecord::UsersRead => Scope::UsersRead,
            ScopeRecord::UsersWrite => Scope::UsersWrite,
            ScopeRecord::AccessTokensRead => Scope::AccessTokensRead,
            ScopeRecord::AccessTokensWrite => Scope::AccessTokensWrite,
            ScopeRecord::SearchFiltersWrite => Scope::SearchFiltersWrite,
            ScopeRecord::WatchlistRead => Scope::WatchlistRead,
            ScopeRecord::WatchlistWrite => Scope::WatchlistWrite,
        }
    }
}
