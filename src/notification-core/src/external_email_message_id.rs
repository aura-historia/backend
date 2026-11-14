domain_primitives::object_id_newtype!(ExternalEmailMessageId, "xem");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_use_xem_prefix_for_external_email_message_id()
    -> Result<(), domain_primitives::object_id::ObjectIdError> {
        let id = ExternalEmailMessageId::new();

        assert_eq!("xem", ExternalEmailMessageId::PREFIX);
        assert!(id.to_string().starts_with("xem_"));
        assert_eq!(id, id.to_string().parse::<ExternalEmailMessageId>()?);
        assert!(
            crate::notification_delivery_id::NotificationDeliveryId::new()
                .to_string()
                .parse::<ExternalEmailMessageId>()
                .is_err()
        );
        Ok(())
    }
}
