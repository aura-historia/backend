use super::NotificationData;
use domain_primitives::object_id::ObjectIdError;
use localization::Language;
use notification_core::notification_id::NotificationId;
use notification_service::{
    ports::notification_list_reader::NotificationListCursor,
    use_cases::queries::list_notifications::{ListNotificationsRequest, ListNotificationsResult},
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use time::{OffsetDateTime, format_description::well_known::Rfc3339};

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ListNotificationsQuery {
    #[serde(default, with = "crate::wire::language")]
    #[schemars(schema_with = "crate::wire::language::schema")]
    pub language: Language,
    /// Defaults to 21 and is clamped to 1 through 100, as on REST.
    pub size: Option<u64>,
    /// JSON string of the returned [created, notificationId] array.
    pub search_after: Option<String>,
}

impl ListNotificationsQuery {
    pub fn into_request(self) -> Result<ListNotificationsRequest, InvalidNotificationCursor> {
        Ok(ListNotificationsRequest {
            languages: vec![self.language],
            cursor: self
                .search_after
                .as_deref()
                .map(parse_notification_cursor)
                .transpose()?,
            limit: self.size.unwrap_or(21).clamp(1, 100) as u32,
        })
    }
}

#[derive(Debug, thiserror::Error)]
pub enum InvalidNotificationCursor {
    #[error("{0}")]
    Json(#[from] serde_json::Error),
    #[error("searchAfter must be a JSON array containing timestamp and notification ID.")]
    NotArray,
    #[error("searchAfter must contain an RFC3339 timestamp and Notification ID.")]
    InvalidMembers,
    #[error("{0}")]
    Timestamp(#[from] time::error::Parse),
    #[error("must be a valid Notification ID")]
    NotificationId(#[source] ObjectIdError),
}

pub fn parse_notification_cursor(
    value: &str,
) -> Result<NotificationListCursor, InvalidNotificationCursor> {
    let Value::Array(values) = serde_json::from_str::<Value>(value)? else {
        return Err(InvalidNotificationCursor::NotArray);
    };
    let [Value::String(created), Value::String(notification_id)] = values.as_slice() else {
        return Err(InvalidNotificationCursor::InvalidMembers);
    };
    Ok(NotificationListCursor {
        created: OffsetDateTime::parse(created, &Rfc3339)?,
        notification_id: notification_id
            .parse::<NotificationId>()
            .map_err(InvalidNotificationCursor::NotificationId)?,
    })
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct NotificationsPageData {
    items: Vec<NotificationData>,
    #[schemars(range(max = u64::MAX))]
    size: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(schema_with = "cursor_schema")]
    search_after: Option<Value>,
}

impl TryFrom<ListNotificationsResult> for NotificationsPageData {
    type Error = time::error::Format;

    fn try_from(result: ListNotificationsResult) -> Result<Self, Self::Error> {
        let search_after = result
            .next_cursor
            .map(|cursor| {
                cursor
                    .created
                    .format(&Rfc3339)
                    .map(|created| json!([created, cursor.notification_id]))
            })
            .transpose()?;
        let items: Vec<_> = result
            .items
            .into_iter()
            .map(|item| NotificationData::from((item, result.presentation_preferences)))
            .collect();
        Ok(Self {
            size: items.len() as u64,
            items,
            search_after,
        })
    }
}

fn cursor_schema(generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
    schemars::json_schema!({"type":"array","prefixItems":[crate::wire::schema::datetime(generator),crate::wire::schema::notification_id(generator)],"minItems":2,"maxItems":2})
}
