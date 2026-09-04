#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SortListingSourceField {
    #[default]
    Name,
    Slug,
    Created,
    Updated,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_default_sort_listing_source_field_to_name() {
        assert_eq!(
            SortListingSourceField::Name,
            SortListingSourceField::default()
        );
    }
}
