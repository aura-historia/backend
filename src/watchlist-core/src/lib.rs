use crate::watchlist_state::WatchlistState;
use product_core::product_id::ProductId;
use user_core::user_id::UserId;

pub mod watchlist_state;

#[derive(Debug, Clone, PartialEq)]
pub struct WatchlistProduct {
    user_id: UserId,
    product_id: ProductId,
    notifications: bool,
    state: WatchlistState,
}

#[derive(Debug, Clone, PartialEq)]
pub struct NewWatchlistProduct {
    pub user_id: UserId,
    pub product_id: ProductId,
    pub notifications: bool,
    pub state: WatchlistState,
}

impl WatchlistProduct {
    pub fn create(new: NewWatchlistProduct) -> Self {
        Self {
            user_id: new.user_id,
            product_id: new.product_id,
            notifications: new.notifications,
            state: new.state,
        }
    }

    pub fn rehydrate(
        user_id: UserId,
        product_id: ProductId,
        notifications: bool,
        state: WatchlistState,
    ) -> Self {
        Self {
            user_id,
            product_id,
            notifications,
            state,
        }
    }

    pub fn change_notifications(&mut self, notifications: bool) {
        self.notifications = notifications;
    }

    pub fn change_state(&mut self, state: WatchlistState) {
        self.state = state;
    }

    pub fn user_id(&self) -> UserId {
        self.user_id
    }
    pub fn product_id(&self) -> ProductId {
        self.product_id
    }
    pub fn notifications(&self) -> bool {
        self.notifications
    }
    pub fn state(&self) -> WatchlistState {
        self.state
    }
}

#[cfg(feature = "test-data")]
mod faker {
    use super::*;
    use fake::{Dummy, Fake, Faker, RngExt};

    impl Dummy<Faker> for WatchlistProduct {
        fn dummy_with_rng<R: RngExt + ?Sized>(config: &Faker, rng: &mut R) -> Self {
            Self::rehydrate(
                config.fake_with_rng(rng),
                config.fake_with_rng(rng),
                config.fake_with_rng(rng),
                WatchlistState::Active,
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_create_active_watchlist_product() {
        let user_id = UserId::new();
        let product_id = ProductId::new();

        let entry = WatchlistProduct::create(NewWatchlistProduct {
            user_id,
            product_id,
            notifications: true,
            state: WatchlistState::Active,
        });

        assert_eq!(user_id, entry.user_id());
        assert_eq!(product_id, entry.product_id());
        assert!(entry.notifications());
        assert_eq!(WatchlistState::Active, entry.state());
    }

    #[test]
    fn should_change_notifications() {
        let mut entry = WatchlistProduct::create(NewWatchlistProduct {
            user_id: UserId::new(),
            product_id: ProductId::new(),
            notifications: true,
            state: WatchlistState::Active,
        });

        entry.change_notifications(false);

        assert!(!entry.notifications());
    }

    #[test]
    fn should_change_state() {
        let mut entry = WatchlistProduct::create(NewWatchlistProduct {
            user_id: UserId::new(),
            product_id: ProductId::new(),
            notifications: true,
            state: WatchlistState::Active,
        });

        entry.change_state(WatchlistState::InactiveByUser);

        assert_eq!(WatchlistState::InactiveByUser, entry.state());
    }
}
