domain_primitives::object_id_newtype!(OAuthConnectionId, "ocn");

#[cfg(test)]
mod tests {
    use super::OAuthConnectionId;

    #[test]
    fn should_generate_uuid_v7_oauth_connection_id_with_ocn_prefix() {
        let id = OAuthConnectionId::new();

        assert_eq!("ocn", OAuthConnectionId::PREFIX);
        assert_eq!(7, id.as_uuid().get_version_num());
        assert!(id.to_string().starts_with("ocn_"));
        assert_eq!(Ok(id), OAuthConnectionId::try_from(id.to_string()));
    }
}
