domain_primitives::object_id_newtype!(MarketingConsentSyncIntentId, "mci");

#[cfg(test)]
mod tests {
    use super::MarketingConsentSyncIntentId;
    use uuid::Uuid;

    #[test]
    fn uses_canonical_mci_uuid_v7_typeid() {
        let id = MarketingConsentSyncIntentId::new();
        assert_eq!(7, id.as_uuid().get_version_num());
        assert!(id.to_string().starts_with("mci_"));
        assert_eq!(
            id,
            MarketingConsentSyncIntentId::try_from(*id.as_uuid()).unwrap()
        );
        assert!(MarketingConsentSyncIntentId::try_from(Uuid::new_v4()).is_err());
        assert!(MarketingConsentSyncIntentId::try_from(id.as_uuid().to_string()).is_err());
    }
}
