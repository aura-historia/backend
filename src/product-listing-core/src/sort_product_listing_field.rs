#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SortProductListingField {
    #[default]
    Score,
    Updated,
    Created,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_default_to_score() {
        assert_eq!(
            SortProductListingField::Score,
            SortProductListingField::default()
        );
    }

    #[rstest::rstest]
    #[case(SortProductListingField::Score)]
    #[case(SortProductListingField::Updated)]
    #[case(SortProductListingField::Created)]
    fn should_clone_and_compare_all_variants(#[case] field: SortProductListingField) {
        let cloned = field;

        assert_eq!(field, cloned);
    }
}
