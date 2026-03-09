use crate::core::notification_id::NotificationId;
use common::user_id::UserId;
use serde::{Deserialize, Serialize};
use serde_fields::SerdeField;
use time::OffsetDateTime;

// #[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)] // fine as related Notification[TYPE]TypeRecord disjoint - their contructors are globally unique
pub enum NotificationRecord {
    Watchlist(NotificationWatchlistRecord),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, SerdeField)]
pub struct NotificationWatchlistRecord {
    pub pk: String,
    pub sk: String,
    pub user_id: UserId,
    pub notification_id: NotificationId,
    pub notification_type: NotificationWatchlistTypeRecord,
    pub seen: bool,

    #[serde(with = "time::serde::rfc3339")]
    pub created: OffsetDateTime,

    #[serde(with = "time::serde::rfc3339")]
    pub updated: OffsetDateTime,
}

pub fn mk_pk(user_id: &UserId) -> String {
    format!("user#{user_id}")
}

pub fn mk_sk(notification_id: &NotificationId) -> String {
    format!("user#notification#{notification_id}")
}

#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum NotificationWatchlistTypeRecord {
    StateListed,
    StateAvailable,
    StateReserved,
    StateSold,
    StateRemoved,
    StateUnknown,
    PriceDiscovered,
    PriceDropped,
    PriceIncreased,
    PriceRemoved,
}
