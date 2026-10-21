use super::errors;
use crate::OperationError;
use application::error::static_error;
use notification_service::{
    ports::{
        notification_deleter::NotificationDeleteError,
        notification_list_reader::NotificationListReadError,
        notification_seen_writer::NotificationSeenWriteError,
    },
    use_cases::{
        commands::{
            delete_notification::DeleteNotificationError,
            delete_notifications::DeleteNotificationsError,
            update_all_notifications_seen::UpdateAllNotificationsSeenError,
            update_notification_seen::UpdateNotificationSeenError,
            update_notifications_seen::UpdateNotificationsSeenError,
        },
        queries::list_notifications::ListNotificationsError,
    },
};
use std::error::Error;

fn seen_error() -> NotificationSeenWriteError {
    NotificationSeenWriteError::UpdateFailed {
        source: static_error("private write cause"),
    }
}
fn delete_error() -> NotificationDeleteError {
    NotificationDeleteError::DeleteFailed {
        source: static_error("private delete cause"),
    }
}
#[test]
fn should_preserve_technical_failure_causes_for_all_six_use_cases() {
    let errors = [
        errors::list(ListNotificationsError::ReadFailed(
            NotificationListReadError::ReadFailed {
                source: static_error("private read cause"),
            },
        )),
        errors::update_one(UpdateNotificationSeenError::UpdateFailed(seen_error())),
        errors::update_selected(UpdateNotificationsSeenError::UpdateFailed(seen_error())),
        errors::update_all(UpdateAllNotificationsSeenError::UpdateFailed(seen_error())),
        errors::delete_one(DeleteNotificationError::DeleteFailed(delete_error())),
        errors::delete_all(DeleteNotificationsError::DeleteFailed(delete_error())),
    ];
    for error in errors {
        assert!(matches!(error, OperationError::Unavailable(_)));
        let mut cause = error.source().unwrap();
        while let Some(source) = cause.source() {
            cause = source;
        }
        assert!(cause.to_string().starts_with("private "));
    }
    let invalid = errors::list(ListNotificationsError::ReadFailed(
        NotificationListReadError::InvalidReadModel {
            source: static_error("invalid state"),
        },
    ));
    assert!(matches!(invalid, OperationError::Internal(_)));
    assert_eq!("invalid state", invalid.source().unwrap().to_string());
}

fn assert_argument_schema<T>(valid: &[serde_json::Value], invalid: &[serde_json::Value])
where
    T: serde::de::DeserializeOwned + schemars::JsonSchema,
{
    let schema = serde_json::to_value(
        schemars::generate::SchemaSettings::default()
            .for_deserialize()
            .into_generator()
            .into_root_schema_for::<T>(),
    )
    .unwrap();
    let validator = jsonschema::validator_for(&schema).unwrap();
    for value in valid {
        assert!(
            serde_json::from_value::<T>(value.clone()).is_ok(),
            "{value}"
        );
        assert!(validator.is_valid(value), "{value}: {schema}");
    }
    for value in invalid {
        assert!(
            serde_json::from_value::<T>(value.clone()).is_err(),
            "{value}"
        );
        assert!(!validator.is_valid(value), "{value}: {schema}");
    }
}

#[test]
fn should_describe_real_inputs_without_accepting_invented_ownership_or_delete_selection() {
    use super::{
        AllSeenArguments, EmptyArguments, ListArguments, NotificationArguments,
        SelectedSeenArguments, SingleSeenArguments,
    };
    use notification_core::notification_id::NotificationId;
    use serde_json::json;
    let id = NotificationId::new();
    assert_argument_schema::<ListArguments>(
        &[
            json!({}),
            json!({"language":"de-DE","size":0,"searchAfter":"[]"}),
        ],
        &[
            json!({"userId":"foreign"}),
            json!({"language":"xx"}),
            json!({"size":-1}),
        ],
    );
    assert_argument_schema::<SelectedSeenArguments>(
        &[
            json!({"notificationIds":[id,id],"seen":false}),
            json!({"notificationIds":[],"seen":true}),
        ],
        &[
            json!({"notificationIds":[id]}),
            json!({"notificationIds":[id],"seen":null}),
            json!({"notificationIds":[id],"seen":true,"userId":"foreign"}),
        ],
    );
    assert_argument_schema::<SingleSeenArguments>(
        &[json!({"notificationId":id,"seen":true})],
        &[
            json!({"notificationId":"bad","seen":true}),
            json!({"notificationId":id,"seen":true,"notificationIds":[id]}),
        ],
    );
    assert_argument_schema::<AllSeenArguments>(
        &[json!({"seen":false})],
        &[json!({"seen":true,"notificationIds":[id]}), json!({})],
    );
    assert_argument_schema::<NotificationArguments>(
        &[json!({"notificationId":id})],
        &[
            json!({"notificationId":"bad"}),
            json!({"notificationIds":[id]}),
        ],
    );
    assert_argument_schema::<EmptyArguments>(
        &[json!({})],
        &[
            json!({"notificationIds":[id]}),
            json!({"body":{}}),
            json!({"seen":true}),
        ],
    );
}
