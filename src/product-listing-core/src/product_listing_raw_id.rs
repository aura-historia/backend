domain_primitives::object_id_newtype!(ProductListingRawStreamId, "prs");
domain_primitives::object_id_newtype!(ProductListingRawRevisionId, "prr");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn raw_ids_keep_their_canonical_prefixes() {
        assert_eq!("prs", ProductListingRawStreamId::PREFIX);
        assert_eq!("prr", ProductListingRawRevisionId::PREFIX);
        assert!(
            ProductListingRawStreamId::new()
                .to_string()
                .starts_with("prs_")
        );
        assert!(
            ProductListingRawRevisionId::new()
                .to_string()
                .starts_with("prr_")
        );
    }
}
