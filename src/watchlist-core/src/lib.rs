pub use crate::watchlist_state::WatchlistState;
use product_listing_core::product_listing_id::ProductListingId;
use user_core::user_id::UserId;

pub mod watchlist_state;

#[derive(Debug, Clone, PartialEq)]
pub struct WatchlistProductListing {
    user_id: UserId,
    product_listing_id: ProductListingId,
    notifications: bool,
    state: WatchlistState,
}

#[derive(Debug, Clone, PartialEq)]
pub struct NewWatchlistProductListing {
    pub user_id: UserId,
    pub product_listing_id: ProductListingId,
    pub notifications: bool,
    pub state: WatchlistState,
}

impl WatchlistProductListing {
    pub fn create(new: NewWatchlistProductListing) -> Self {
        Self {
            user_id: new.user_id,
            product_listing_id: new.product_listing_id,
            notifications: new.notifications,
            state: new.state,
        }
    }

    pub fn rehydrate(
        user_id: UserId,
        product_listing_id: ProductListingId,
        notifications: bool,
        state: WatchlistState,
    ) -> Self {
        Self {
            user_id,
            product_listing_id,
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
    pub fn product_listing_id(&self) -> ProductListingId {
        self.product_listing_id
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

    impl Dummy<Faker> for WatchlistProductListing {
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
        let product_listing_id = ProductListingId::new();

        let entry = WatchlistProductListing::create(NewWatchlistProductListing {
            user_id,
            product_listing_id,
            notifications: true,
            state: WatchlistState::Active,
        });

        assert_eq!(user_id, entry.user_id());
        assert_eq!(product_listing_id, entry.product_listing_id());
        assert!(entry.notifications());
        assert_eq!(WatchlistState::Active, entry.state());
    }

    #[test]
    fn should_change_notifications() {
        let mut entry = WatchlistProductListing::create(NewWatchlistProductListing {
            user_id: UserId::new(),
            product_listing_id: ProductListingId::new(),
            notifications: true,
            state: WatchlistState::Active,
        });

        entry.change_notifications(false);

        assert!(!entry.notifications());
    }

    #[test]
    fn should_change_state() {
        let mut entry = WatchlistProductListing::create(NewWatchlistProductListing {
            user_id: UserId::new(),
            product_listing_id: ProductListingId::new(),
            notifications: true,
            state: WatchlistState::Active,
        });

        entry.change_state(WatchlistState::InactiveByUser);

        assert_eq!(WatchlistState::InactiveByUser, entry.state());
    }
}
