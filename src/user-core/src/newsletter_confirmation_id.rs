domain_primitives::object_id_newtype!(NewsletterConfirmationId, "nsc");

#[cfg(test)]
mod tests {
    use super::NewsletterConfirmationId;
    use uuid::Uuid;

    #[test]
    fn uses_canonical_nsc_uuid_v7_typeid() {
        let id = NewsletterConfirmationId::new();

        assert_eq!(7, id.as_uuid().get_version_num());
        assert!(id.to_string().starts_with("nsc_"));
        assert_eq!(
            id,
            NewsletterConfirmationId::try_from(*id.as_uuid()).unwrap()
        );
        assert!(NewsletterConfirmationId::try_from(Uuid::new_v4()).is_err());
    }
}
