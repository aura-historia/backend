use common::product_id::ProductId;
use common::resource_state::domain::ResourceState;
use common::user_id::UserId;
use common::versioned::Versioned;
use sqlx::FromRow;
use watchlist_core::WatchlistProduct;
use watchlist_service::ports::{
    VersionedWatchlistProduct, WatchlistProductView, WatchlistReadError, WatchlistRepositoryError,
    WatchlistStorageVersion,
};

#[derive(FromRow)]
pub(crate) struct WatchlistRepositoryRow {
    pub user_id: uuid::Uuid,
    pub product_id: uuid::Uuid,
    pub notifications: bool,
    pub state: String,
    pub version: i64,
}

#[derive(FromRow)]
pub(crate) struct WatchlistViewRow {
    pub user_id: uuid::Uuid,
    pub product_id: uuid::Uuid,
    pub notifications: bool,
    pub state: String,
    pub created: time::OffsetDateTime,
    pub updated: time::OffsetDateTime,
}

impl TryFrom<WatchlistRepositoryRow> for VersionedWatchlistProduct {
    type Error = WatchlistRepositoryError;

    fn try_from(row: WatchlistRepositoryRow) -> Result<Self, Self::Error> {
        let version = WatchlistStorageVersion::try_from(row.version)
            .map_err(|_| WatchlistRepositoryError::InvalidPersistedState)?;
        let entry = WatchlistProduct::rehydrate(
            UserId::from(row.user_id),
            ProductId::from(row.product_id),
            row.notifications,
            parse_state_repository(&row.state)?,
        );
        Ok(Versioned::new(entry, version))
    }
}

impl TryFrom<WatchlistViewRow> for WatchlistProductView {
    type Error = WatchlistReadError;

    fn try_from(row: WatchlistViewRow) -> Result<Self, Self::Error> {
        Ok(Self {
            user_id: UserId::from(row.user_id),
            product_id: ProductId::from(row.product_id),
            notifications: row.notifications,
            state: parse_state_read(&row.state)?,
            created: row.created,
            updated: row.updated,
        })
    }
}

pub(crate) fn format_state(state: ResourceState) -> &'static str {
    match state {
        ResourceState::Active => "ACTIVE",
        ResourceState::InactiveByUser => "INACTIVE_BY_USER",
        ResourceState::InactiveByRestrictedPlan => "INACTIVE_BY_RESTRICTED_PLAN",
    }
}

fn parse_state(value: &str) -> Option<ResourceState> {
    match value {
        "ACTIVE" => Some(ResourceState::Active),
        "INACTIVE_BY_USER" => Some(ResourceState::InactiveByUser),
        "INACTIVE_BY_RESTRICTED_PLAN" => Some(ResourceState::InactiveByRestrictedPlan),
        _ => None,
    }
}

fn parse_state_repository(value: &str) -> Result<ResourceState, WatchlistRepositoryError> {
    parse_state(value).ok_or(WatchlistRepositoryError::InvalidPersistedState)
}

fn parse_state_read(value: &str) -> Result<ResourceState, WatchlistReadError> {
    parse_state(value).ok_or(WatchlistReadError::InvalidPersistedState)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_format_all_states() {
        assert_eq!("ACTIVE", format_state(ResourceState::Active));
        assert_eq!(
            "INACTIVE_BY_USER",
            format_state(ResourceState::InactiveByUser)
        );
        assert_eq!(
            "INACTIVE_BY_RESTRICTED_PLAN",
            format_state(ResourceState::InactiveByRestrictedPlan)
        );
    }

    #[test]
    fn should_reject_invalid_state() {
        assert!(matches!(
            parse_state_repository("bad"),
            Err(WatchlistRepositoryError::InvalidPersistedState)
        ));
    }

    #[test]
    fn should_reject_zero_or_negative_repository_version() {
        for version in [0, -1] {
            let row = WatchlistRepositoryRow {
                user_id: uuid::Uuid::new_v4(),
                product_id: uuid::Uuid::new_v4(),
                notifications: true,
                state: "ACTIVE".to_owned(),
                version,
            };

            assert!(matches!(
                VersionedWatchlistProduct::try_from(row),
                Err(WatchlistRepositoryError::InvalidPersistedState)
            ));
        }
    }
}
