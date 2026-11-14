//! Storage of external emails: messages to addresses that do not belong to a user account.

mod delivery_source;
mod mapping;
mod outbox;

pub(crate) use delivery_source::load_delivery_source;
pub use outbox::SqlxExternalEmailOutboxFactory;
