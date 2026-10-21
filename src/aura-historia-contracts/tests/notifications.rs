use aura_historia_contracts::notifications::{
    InvalidNotificationCursor, ListNotificationsQuery, NotificationsPageData,
    parse_notification_cursor,
};
use localization::Language;
use notification_core::notification_id::NotificationId;
use notification_service::{
    ports::notification_list_reader::NotificationListCursor,
    presentation::NotificationPresentationPreferences,
    use_cases::queries::list_notifications::ListNotificationsResult,
};
use schemars::generate::SchemaSettings;
use serde_json::{Value, json};
use time::macros::datetime;

#[test]
fn should_preserve_defaults_language_aliases_clamped_limits_and_cursor_validation() {
    let schema = serde_json::to_value(
        SchemaSettings::default()
            .for_deserialize()
            .into_generator()
            .into_root_schema_for::<ListNotificationsQuery>(),
    )
    .unwrap();
    assert_eq!(
        Some(u64::MAX),
        schema["properties"]["size"]["maximum"].as_u64()
    );
    let id = NotificationId::new();
    let cursor = json!(["2026-10-21T10:00:00Z", id]).to_string();
    for (size, expected) in [
        (None, 21),
        (Some(0), 1),
        (Some(1), 1),
        (Some(100), 100),
        (Some(u64::MAX), 100),
    ] {
        let mut value = json!({"language":"en-GB","searchAfter":cursor});
        if let Some(size) = size {
            value["size"] = json!(size);
        }
        let query: ListNotificationsQuery = serde_json::from_value(value).unwrap();
        let request = query.into_request().unwrap();
        assert_eq!(vec![Language::En], request.languages);
        assert_eq!(expected, request.limit);
        assert_eq!(id, request.cursor.unwrap().notification_id);
    }
    let query: ListNotificationsQuery = serde_json::from_value(json!({})).unwrap();
    assert_eq!(vec![Language::En], query.into_request().unwrap().languages);
    for value in [
        "null",
        "{}",
        "[]",
        "[1,2]",
        "[\"2026-10-21T10:00:00Z\",\"bad\"]",
        "[\"bad\",\"bad\"]",
        "[",
    ] {
        assert!(parse_notification_cursor(value).is_err(), "{value}");
    }
    assert!(matches!(
        parse_notification_cursor("[\"2026-10-21T10:00:00Z\",\"bad\"]"),
        Err(InvalidNotificationCursor::NotificationId(_))
    ));
}

#[test]
fn should_serialize_exact_empty_page_and_optional_cursor_as_described_by_schema() {
    let schema = serde_json::to_value(
        SchemaSettings::default()
            .for_serialize()
            .into_generator()
            .into_root_schema_for::<NotificationsPageData>(),
    )
    .unwrap();
    let validator = jsonschema::validator_for(&schema).unwrap();
    for next in [
        None,
        Some(NotificationListCursor {
            created: datetime!(2026-10-21 10:00 UTC),
            notification_id: NotificationId::new(),
        }),
    ] {
        let page = NotificationsPageData::try_from(ListNotificationsResult {
            items: vec![],
            next_cursor: next,
            presentation_preferences: NotificationPresentationPreferences {
                language: Language::En,
                show_unassessed_or_sensitive_content: false,
            },
        })
        .unwrap();
        let value = serde_json::to_value(page).unwrap();
        assert!(validator.is_valid(&value), "{value}");
        assert_eq!(Value::from(0), value["size"]);
        assert_eq!(json!([]), value["items"]);
        assert!(value.get("total").is_none());
        if let Some(next) = next {
            assert_eq!(
                next,
                parse_notification_cursor(&value["searchAfter"].to_string()).unwrap()
            );
        } else {
            assert_eq!(json!({"items":[],"size":0}), value);
        }
    }
}
