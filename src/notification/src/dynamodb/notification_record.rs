use crate::{
    core::notification_id::NotificationId,
    dynamodb::{
        notification_reason_record::NotificationReasonRecord,
        notification_type_record::NotificationTypeRecord,
    },
};
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
    pub notification_type: NotificationTypeRecord,
    pub notification_reason: NotificationReasonRecord,
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
