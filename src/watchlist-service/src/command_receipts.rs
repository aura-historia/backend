use crate::use_cases::{
    UnwatchProductListingResult, UpdateWatchlistProductListingResult, WatchProductListingResult,
};
use application::{
    error::{BoxError, static_error},
    idempotency::{CommandOutcome, field},
};
use serde_json::{Value, json};
use watchlist_core::{WatchlistProductListing, WatchlistState};

fn encode(entry: &WatchlistProductListing) -> Value {
    json!({"user_id":entry.user_id(),"product_listing_id":entry.product_listing_id(),"notifications":entry.notifications(),"state":entry.state().as_str()})
}
fn decode(v: Value) -> Result<WatchlistProductListing, BoxError> {
    let state = WatchlistState::from_code(&field::<String>(&v, "state")?)
        .ok_or_else(|| static_error("invalid watchlist receipt state"))?;
    Ok(WatchlistProductListing::rehydrate(
        field(&v, "user_id")?,
        field(&v, "product_listing_id")?,
        field(&v, "notifications")?,
        state,
    ))
}
macro_rules! entry_outcome {
    ($ty:ty) => {
        impl CommandOutcome for $ty {
            fn private_owner(&self) -> Option<user_core::user_id::UserId> {
                Some(self.entry.user_id())
            }
            fn encode(&self) -> Result<Value, BoxError> {
                Ok(encode(&self.entry))
            }
            fn decode(v: Value) -> Result<Self, BoxError> {
                Ok(Self { entry: decode(v)? })
            }
        }
    };
}
entry_outcome!(WatchProductListingResult);
entry_outcome!(UpdateWatchlistProductListingResult);
application::command_outcome!(UnwatchProductListingResult {
    user_id,
    product_listing_id
});
