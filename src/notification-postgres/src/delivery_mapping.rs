use notification_core::notification_delivery::NotificationDeliveryChannel;

pub(crate) const fn channel_to_persisted(channel: NotificationDeliveryChannel) -> &'static str {
    match channel {
        NotificationDeliveryChannel::Email => "EMAIL",
    }
}

pub(crate) fn channel_from_persisted(
    value: &str,
) -> Result<NotificationDeliveryChannel, InvalidPersistedChannel> {
    match value {
        "EMAIL" => Ok(NotificationDeliveryChannel::Email),
        _ => Err(InvalidPersistedChannel(value.to_owned())),
    }
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
#[error("unknown notification delivery channel {0}")]
pub(crate) struct InvalidPersistedChannel(String);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_map_email_channel_to_postgres_value() {
        assert_eq!(
            "EMAIL",
            channel_to_persisted(NotificationDeliveryChannel::Email)
        );
    }

    #[test]
    fn should_map_postgres_email_value_to_channel() {
        assert_eq!(
            NotificationDeliveryChannel::Email,
            channel_from_persisted("EMAIL").expect("EMAIL should be valid")
        );
    }

    #[test]
    fn should_reject_unknown_postgres_channel_value() {
        assert!(channel_from_persisted("SMS").is_err());
    }
}
