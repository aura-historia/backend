#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum NotificationDeliveryChannel {
    Email,
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
        let normalized = value.trim();
        if normalized.is_empty() {
            return Err(InvalidNotificationDeliveryTargetKey);
        }
        Ok(Self(normalized.to_owned()))
    }
}

#[derive(Debug, thiserror::Error)]
#[error("notification delivery target key must not be blank")]
pub struct InvalidNotificationDeliveryTargetKey;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_normalize_surrounding_target_key_whitespace() {
        assert_eq!(
            "PRIMARY",
            NotificationDeliveryTargetKey::try_from("  PRIMARY \t".to_owned())
                .expect("target key should be valid")
                .as_str()
        );
    }

    #[test]
    fn should_reject_blank_target_key() {
        assert!(NotificationDeliveryTargetKey::try_from(" \t ".to_owned()).is_err());
    }
}
