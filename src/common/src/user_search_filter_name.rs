// Legacy shim. Owner: search-filter-core. Remove after legacy common consumers migrate.
pub use search_filter_core::user_search_filter_name::UserSearchFilterName;

#[cfg(test)]
mod compatibility_tests {
    use super::UserSearchFilterName;

    #[test]
    fn should_preserve_name_truncation() {
        let name = UserSearchFilterName::from("a".repeat(300));
        assert_eq!(255, name.as_ref().len());
    }
}
