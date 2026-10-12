domain_primitives::object_id_newtype!(PartyLocationId, "ploc");

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn strict_ploc_identity_roundtrips_and_rejects_foreign_ids() {
        let id = PartyLocationId::new();
        assert_eq!("ploc", PartyLocationId::PREFIX);
        assert_eq!(id, id.to_string().parse().unwrap());
        assert_eq!(id, PartyLocationId::try_from(id.into_uuid()).unwrap());
        assert!(PartyLocationId::try_from(uuid::Uuid::new_v4()).is_err());
        assert!(PartyLocationId::try_from(uuid::Uuid::nil()).is_err());
        assert!(id.as_uuid().to_string().parse::<PartyLocationId>().is_err());
        assert!(
            id.to_string()
                .replace("ploc_", "pty_")
                .parse::<PartyLocationId>()
                .is_err()
        );
    }
}
