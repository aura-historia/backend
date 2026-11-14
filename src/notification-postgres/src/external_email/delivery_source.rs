use super::mapping::decode_content;
use crate::delivery_mapping::channel_from_persisted;
use application::error::box_error;
use localization::Language;
use notification_core::{
    external_email::ExternalEmailRecipient,
    external_email_message_id::ExternalEmailMessageId,
    notification_delivery::{NotificationDeliveryChannel, NotificationDeliveryTargetKey},
    notification_delivery_id::NotificationDeliveryId,
};
use notification_service::ports::notification_delivery_repository::{
    ExternalEmailDeliverySource, NotificationDeliveryError, NotificationDeliverySource,
};
use sqlx::PgConnection;
use uuid::Uuid;

/// Holds the raw recipient address and payload, so it has no `Debug`.
#[derive(sqlx::FromRow)]
pub(super) struct ExternalEmailSourceRow {
    channel: String,
    target_key: String,
    external_email_message_id: Uuid,
    recipient_email: String,
    language: String,
    template: String,
    payload_version: i32,
    payload: serde_json::Value,
}

/// Loads the message of a claimed external delivery without reading any user account.
/// The message is mandatory: the foreign key cascades its erasure to the delivery, so a
/// delivery without its message, like an undecodable message, is invalid persisted state.
pub(crate) async fn load_delivery_source(
    connection: &mut PgConnection,
    notification_delivery_id: NotificationDeliveryId,
) -> Result<NotificationDeliverySource, NotificationDeliveryError> {
    let row = sqlx::query_as::<_, ExternalEmailSourceRow>(
        "SELECT d.channel, d.target_key, m.external_email_message_id, m.recipient_email, m.language, m.template, m.payload_version, m.payload FROM notification_deliveries d JOIN external_email_messages m USING (external_email_message_id) WHERE d.notification_delivery_id = $1",
    )
    .bind(notification_delivery_id.into_uuid())
    .fetch_optional(connection)
    .await
    .map_err(|source| NotificationDeliveryError::OperationFailed {
        source: box_error(source),
    })?;
    source_from_row(notification_delivery_id, row).map(NotificationDeliverySource::ExternalEmail)
}

fn source_from_row(
    notification_delivery_id: NotificationDeliveryId,
    row: Option<ExternalEmailSourceRow>,
) -> Result<ExternalEmailDeliverySource, NotificationDeliveryError> {
    let row = row.ok_or_else(|| invalid("external email message is missing"))?;
    if channel_from_persisted(&row.channel).map_err(invalid_source)?
        != NotificationDeliveryChannel::Email
        || row.target_key != NotificationDeliveryTargetKey::recipient().as_str()
    {
        return Err(invalid("external email delivery target is invalid"));
    }
    Ok(ExternalEmailDeliverySource {
        notification_delivery_id,
        external_email_message_id: ExternalEmailMessageId::try_from(row.external_email_message_id)
            .map_err(invalid_source)?,
        recipient: ExternalEmailRecipient::try_from(row.recipient_email).map_err(invalid_source)?,
        language: Language::from_code(&row.language)
            .ok_or_else(|| invalid("unknown external email language"))?,
        content: decode_content(&row.template, row.payload_version, row.payload)
            .map_err(invalid_source)?,
    })
}

fn invalid(message: &'static str) -> NotificationDeliveryError {
    invalid_source(std::io::Error::other(message))
}

fn invalid_source(
    source: impl std::error::Error + Send + Sync + 'static,
) -> NotificationDeliveryError {
    NotificationDeliveryError::InvalidPersistedState {
        source: box_error(source),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use notification_core::external_email::{ContentNoticeReceiptTarget, ExternalEmailContent};
    use serde_json::json;

    fn row() -> ExternalEmailSourceRow {
        ExternalEmailSourceRow {
            channel: "EMAIL".to_owned(),
            target_key: "RECIPIENT".to_owned(),
            external_email_message_id: ExternalEmailMessageId::new().into_uuid(),
            recipient_email: "notifier@example.test".to_owned(),
            language: "fr".to_owned(),
            template: "CONTENT_NOTICE_RECEIPT".to_owned(),
            payload_version: 1,
            payload: json!({
                "content_notice_id": "01890a5d-ac96-774b-bf1d-d5586c639f75",
                "received_at": "2026-11-14T12:30:05Z",
                "provider_name": "Antik Shop",
                "target_type": "LISTING_SOURCE",
                "listing_title": null,
                "reason_category": "OTHER",
            }),
        }
    }

    #[test]
    fn should_map_the_message_without_a_user_account() -> Result<(), NotificationDeliveryError> {
        let id = NotificationDeliveryId::new();

        let source = source_from_row(id, Some(row()))?;

        assert_eq!(id, source.notification_delivery_id);
        assert_eq!("notifier@example.test", source.recipient.as_str());
        assert_eq!(Language::Fr, source.language);
        let ExternalEmailContent::ContentNoticeReceipt(receipt) = source.content;
        assert_eq!("Antik Shop", receipt.provider_name.as_ref());
        assert_eq!(ContentNoticeReceiptTarget::ListingSource, receipt.target);
        Ok(())
    }

    #[test]
    fn should_fail_closed_on_missing_message_or_invalid_persisted_values() {
        let mut invalid_rows = vec![None];
        for mutate in [
            (|row: &mut ExternalEmailSourceRow| row.channel = "SMS".to_owned())
                as fn(&mut ExternalEmailSourceRow),
            |row| row.target_key = "PRIMARY".to_owned(),
            |row| row.external_email_message_id = Uuid::nil(),
            |row| row.recipient_email = "not-an-address".to_owned(),
            |row| row.language = "xx".to_owned(),
            |row| row.template = "UNKNOWN".to_owned(),
            |row| row.payload = json!({}),
        ] {
            let mut row = row();
            mutate(&mut row);
            invalid_rows.push(Some(row));
        }

        for row in invalid_rows {
            assert!(matches!(
                source_from_row(NotificationDeliveryId::new(), row),
                Err(NotificationDeliveryError::InvalidPersistedState { .. })
            ));
        }
    }
}
