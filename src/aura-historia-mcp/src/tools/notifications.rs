//! Current-user notification inbox operations; application services own scopes and ownership.
mod errors;

use crate::{AccessClass, OperationDefinition, OperationError, OperationPolicy, RegistryError};
use application::operation_context::CredentialCapability;
use aura_historia_contracts::notifications::{
    ListNotificationsQuery, NotificationsPageData, UpdateNotificationSeenData,
    UpdateNotificationsSeenData,
};
use aura_historia_runtime::Notifications;
use notification_core::notification_id::NotificationId;
use notification_service::use_cases::{
    commands::{
        delete_notification::{DeleteNotificationCommand, DeleteNotificationUseCase},
        delete_notifications::{DeleteNotificationsCommand, DeleteNotificationsUseCase},
        update_all_notifications_seen::{
            UpdateAllNotificationsSeenCommand, UpdateAllNotificationsSeenUseCase,
        },
        update_notification_seen::{UpdateNotificationSeenCommand, UpdateNotificationSeenUseCase},
        update_notifications_seen::{
            UpdateNotificationsSeenCommand, UpdateNotificationsSeenUseCase,
        },
    },
    queries::list_notifications::ListNotificationsUseCase,
};
use schemars::JsonSchema;
use serde::Deserialize;
use std::sync::Arc;

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ListArguments {
    #[serde(flatten)]
    query: ListNotificationsQuery,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct SelectedSeenArguments {
    #[serde(flatten)]
    data: UpdateNotificationsSeenData,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct AllSeenArguments {
    #[serde(flatten)]
    data: UpdateNotificationSeenData,
}

#[derive(Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SingleSeenArguments {
    #[schemars(schema_with = "aura_historia_contracts::wire::schema::notification_id")]
    notification_id: NotificationId,
    #[serde(flatten)]
    data: UpdateNotificationSeenData,
}

#[derive(Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct NotificationArguments {
    #[schemars(schema_with = "aura_historia_contracts::wire::schema::notification_id")]
    notification_id: NotificationId,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct EmptyArguments {}

fn read_policy() -> OperationPolicy {
    OperationPolicy {
        access: AccessClass::Protected,
        required_scopes: vec![CredentialCapability::NotificationsRead],
        read_only: true,
        destructive: false,
        idempotent: true,
        open_world: false,
    }
}

fn mutation_policy() -> OperationPolicy {
    OperationPolicy {
        required_scopes: vec![CredentialCapability::NotificationsWrite],
        read_only: false,
        destructive: true,
        ..read_policy()
    }
}

pub fn operations(services: Notifications) -> Result<Vec<OperationDefinition>, RegistryError> {
    Ok(vec![
        list(services.list)?,
        update_one(services.update_notification_seen)?,
        update_selected(services.update_notifications_seen)?,
        update_all(services.update_all_notifications_seen)?,
        delete_one(services.delete_notification)?,
        delete_all(services.delete_notifications)?,
    ])
}

fn list(list: Arc<dyn ListNotificationsUseCase>) -> Result<OperationDefinition, RegistryError> {
    OperationDefinition::new(
        "listNotifications",
        "List your complete notification inbox payloads and listing snapshots in the requested language, honoring your sensitive-content preference. size defaults to 21 and is clamped to 1 through 100. Serialize the returned searchAfter array as a JSON string to continue.",
        read_policy(),
        move |input: ListArguments, invocation| {
            let list = list.clone();
            async move {
                let request = input
                    .query
                    .into_request()
                    .map_err(|error| OperationError::InvalidArguments(Box::new(error)))?;
                let result = invocation
                    .execute(list.execute(&invocation.operation, request))
                    .await?
                    .map_err(errors::list)?;
                NotificationsPageData::try_from(result)
                    .map_err(|error| OperationError::Internal(Box::new(error)))
            }
        },
    )
}

fn update_one(
    update: Arc<dyn UpdateNotificationSeenUseCase>,
) -> Result<OperationDefinition, RegistryError> {
    OperationDefinition::new(
        "updateNotificationSeen",
        "Set seen on one owned notification identified by notificationId. Missing or foreign IDs return not found. Returns result:null on success.",
        mutation_policy(),
        move |input: SingleSeenArguments, invocation| {
            let update = update.clone();
            async move {
                invocation
                    .execute(update.execute(
                        &invocation.operation,
                        UpdateNotificationSeenCommand {
                            notification_id: input.notification_id,
                            seen: input.data.seen,
                        },
                    ))
                    .await?
                    .map_err(errors::update_one)?;
                Ok(())
            }
        },
    )
}

fn update_selected(
    update: Arc<dyn UpdateNotificationsSeenUseCase>,
) -> Result<OperationDefinition, RegistryError> {
    OperationDefinition::new(
        "updateNotificationsSeen",
        "Set seen on selected notificationIds in your inbox. At least one ID is required; duplicates are deduplicated and missing or foreign IDs are ignored. Returns result:null on success without per-item reports.",
        mutation_policy(),
        move |input: SelectedSeenArguments, invocation| {
            let update = update.clone();
            async move {
                let notification_ids = input
                    .data
                    .notification_ids
                    .into_iter()
                    .map(|id| id.parse::<NotificationId>())
                    .collect::<Result<Vec<_>, _>>()
                    .map_err(|error| OperationError::InvalidArguments(Box::new(error)))?;
                invocation
                    .execute(update.execute(
                        &invocation.operation,
                        UpdateNotificationsSeenCommand {
                            notification_ids,
                            seen: input.data.seen,
                        },
                    ))
                    .await?
                    .map_err(errors::update_selected)?;
                Ok(())
            }
        },
    )
}

fn update_all(
    update: Arc<dyn UpdateAllNotificationsSeenUseCase>,
) -> Result<OperationDefinition, RegistryError> {
    OperationDefinition::new(
        "updateAllNotificationsSeen",
        "Set seen to the supplied boolean for every notification in your entire inbox, including items beyond the listed page. Returns result:null on success.",
        mutation_policy(),
        move |input: AllSeenArguments, invocation| {
            let update = update.clone();
            async move {
                invocation
                    .execute(update.execute(
                        &invocation.operation,
                        UpdateAllNotificationsSeenCommand {
                            seen: input.data.seen,
                        },
                    ))
                    .await?
                    .map_err(errors::update_all)?;
                Ok(())
            }
        },
    )
}

fn delete_one(
    delete: Arc<dyn DeleteNotificationUseCase>,
) -> Result<OperationDefinition, RegistryError> {
    OperationDefinition::new(
        "deleteNotification",
        "Permanently delete one owned notification identified by notificationId. Missing or foreign IDs return not found. Returns result:null on success.",
        mutation_policy(),
        move |input: NotificationArguments, invocation| {
            let delete = delete.clone();
            async move {
                invocation
                    .execute(delete.execute(
                        &invocation.operation,
                        DeleteNotificationCommand {
                            notification_id: input.notification_id,
                        },
                    ))
                    .await?
                    .map_err(errors::delete_one)?;
                Ok(())
            }
        },
    )
}

fn delete_all(
    delete: Arc<dyn DeleteNotificationsUseCase>,
) -> Result<OperationDefinition, RegistryError> {
    OperationDefinition::new(
        "deleteNotifications",
        "Permanently delete your entire notification inbox, including items beyond the listed page. Takes no IDs or business body: arguments must be {}. Returns result:null on success.",
        mutation_policy(),
        move |_: EmptyArguments, invocation| {
            let delete = delete.clone();
            async move {
                invocation
                    .execute(delete.execute(&invocation.operation, DeleteNotificationsCommand))
                    .await?
                    .map_err(errors::delete_all)?;
                Ok(())
            }
        },
    )
}

#[cfg(test)]
mod tests;
