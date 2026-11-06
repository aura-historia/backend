use credential_core::scope::Scope;

/// Scopes for an ordinary user's own account data: profile and addresses, saved searches,
/// watchlist and notifications. Authorization-server discovery advertises this vocabulary;
/// administrative and partner scopes stay with administrator-registered clients.
pub const ORDINARY_USER_SCOPES: [Scope; 8] = [
    Scope::UsersRead,
    Scope::UsersWrite,
    Scope::SearchFiltersRead,
    Scope::SearchFiltersWrite,
    Scope::WatchlistRead,
    Scope::WatchlistWrite,
    Scope::NotificationsRead,
    Scope::NotificationsWrite,
];
