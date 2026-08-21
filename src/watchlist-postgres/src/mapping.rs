use product_core::product_id::ProductId;
use sqlx::FromRow;
use user_core::user_id::UserId;
use watchlist_core::WatchlistProduct;
use watchlist_core::watchlist_state::WatchlistState;
use watchlist_service::ports::{
    WatchlistProductView, WatchlistReadError, WatchlistRepositoryError,
};

#[derive(FromRow)]
pub(crate) struct WatchlistRow {
    pub user_id: uuid::Uuid,
    pub product_id: uuid::Uuid,
    pub notifications: bool,
    pub state: String,
    pub created: time::OffsetDateTime,
    pub updated: time::OffsetDateTime,
}

impl WatchlistRow {
    pub(crate) fn into_domain(self) -> Result<WatchlistProduct, WatchlistRepositoryError> {
        Ok(WatchlistProduct::rehydrate(
            UserId::from(self.user_id),
            ProductId::from(self.product_id),
            self.notifications,
            parse_state_repository(&self.state)?,
        ))
    }

    pub(crate) fn into_view(self) -> Result<WatchlistProductView, WatchlistReadError> {
        let state = parse_state_read(&self.state)?;
        Ok(WatchlistProductView {
            user_id: UserId::from(self.user_id),
            product_id: ProductId::from(self.product_id),
            notifications: self.notifications,
            state,
            created: self.created,
            updated: self.updated,
        })
    }
}

pub(crate) fn format_state(state: WatchlistState) -> &'static str {
    match state {
        WatchlistState::Active => "ACTIVE",
        WatchlistState::InactiveByUser => "INACTIVE_BY_USER",
        WatchlistState::InactiveByRestrictedPlan => "INACTIVE_BY_RESTRICTED_PLAN",
    }
}

fn parse_state(value: &str) -> Option<WatchlistState> {
    match value {
        "ACTIVE" => Some(WatchlistState::Active),
        "INACTIVE_BY_USER" => Some(WatchlistState::InactiveByUser),
        "INACTIVE_BY_RESTRICTED_PLAN" => Some(WatchlistState::InactiveByRestrictedPlan),
        _ => None,
    }
}

fn parse_state_repository(value: &str) -> Result<WatchlistState, WatchlistRepositoryError> {
    parse_state(value).ok_or(WatchlistRepositoryError::InvalidPersistedState)
}

fn parse_state_read(value: &str) -> Result<WatchlistState, WatchlistReadError> {
    parse_state(value).ok_or(WatchlistReadError::InvalidPersistedState)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_format_all_states() {
        assert_eq!("ACTIVE", format_state(WatchlistState::Active));
        assert_eq!(
            "INACTIVE_BY_USER",
            format_state(WatchlistState::InactiveByUser)
        );
        assert_eq!(
            "INACTIVE_BY_RESTRICTED_PLAN",
            format_state(WatchlistState::InactiveByRestrictedPlan)
        );
    }

    #[test]
    fn should_reject_invalid_state() {
        assert!(matches!(
            parse_state_repository("bad"),
            Err(WatchlistRepositoryError::InvalidPersistedState)
        ));
    }
}
