#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SortShopField {
    #[default]
    Name,
    Updated,
    Created,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_default_to_name_when_sort_field_not_set() {
        assert_eq!(SortShopField::Name, SortShopField::default());
    }

    #[test]
    fn should_keep_sort_fields_distinct() {
        assert_ne!(SortShopField::Name, SortShopField::Updated);
        assert_ne!(SortShopField::Name, SortShopField::Created);
        assert_ne!(SortShopField::Updated, SortShopField::Created);
    }
}
