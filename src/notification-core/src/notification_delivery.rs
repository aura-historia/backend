#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum NotificationDeliveryChannel {
    Email,
}

impl NotificationDeliveryChannel {
    pub const fn persisted(self) -> &'static str {
        match self {
            Self::Email => "EMAIL",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct NotificationDeliveryTargetKey(String);

impl NotificationDeliveryTargetKey {
    pub fn primary() -> Self {
        Self("PRIMARY".to_owned())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for NotificationDeliveryTargetKey {
    type Error = InvalidNotificationDeliveryTargetKey;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        if value.trim().is_empty() {
            return Err(InvalidNotificationDeliveryTargetKey);
        }
        Ok(Self(value))
    }
}

#[derive(Debug, thiserror::Error)]
#[error("notification delivery target key must not be blank")]
pub struct InvalidNotificationDeliveryTargetKey;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_persist_channels_as_screaming_snake_case() {
        assert_eq!("EMAIL", NotificationDeliveryChannel::Email.persisted());
    }

    #[test]
    fn should_reject_blank_target_key() {
        assert!(NotificationDeliveryTargetKey::try_from(" \t ".to_owned()).is_err());
    }
}
