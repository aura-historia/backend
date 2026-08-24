use domain_primitives::versioned::Versioned;
use product_listing_core::product_id::ProductId;
use sqlx::FromRow;
use user_core::user_id::UserId;
use watchlist_core::WatchlistProduct;
use watchlist_core::WatchlistState;
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

fn parse_state(value: &str) -> Option<WatchlistState> {
    WatchlistState::from_code(value)
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
    use strum::IntoEnumIterator;

    #[test]
    fn should_parse_each_canonical_state() {
        for expected in WatchlistState::iter() {
            assert_eq!(Some(expected), parse_state(expected.as_str()));
        }
    }

    #[test]
    fn should_reject_unknown_and_noncanonical_states() {
        for value in ["bad", "active"] {
            assert!(matches!(
                parse_state_repository(value),
                Err(WatchlistRepositoryError::InvalidPersistedState)
            ));
            assert!(matches!(
                parse_state_read(value),
                Err(WatchlistReadError::InvalidPersistedState)
            ));
        }
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
