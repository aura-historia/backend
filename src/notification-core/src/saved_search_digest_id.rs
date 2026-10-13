domain_primitives::object_id_newtype!(SavedSearchDigestId, "ssd");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn digest_identity_is_canonical_and_distinct_from_notifications() {
        let id = SavedSearchDigestId::new();
        assert!(id.to_string().starts_with("ssd_"));
        assert_eq!(id, id.to_string().parse::<SavedSearchDigestId>().unwrap());
        assert_eq!(id, SavedSearchDigestId::try_from(id.into_uuid()).unwrap());
        assert!(
            id.as_uuid()
                .to_string()
                .parse::<SavedSearchDigestId>()
                .is_err()
        );
        assert!(
            crate::notification_id::NotificationId::new()
                .to_string()
                .parse::<SavedSearchDigestId>()
                .is_err()
        );
        assert!(SavedSearchDigestId::try_from(uuid::Uuid::nil()).is_err());
    }
}
