#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SortProductField {
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
        assert_eq!(SortProductField::Score, SortProductField::default());
    }

    #[rstest::rstest]
    #[case(SortProductField::Score)]
    #[case(SortProductField::Updated)]
    #[case(SortProductField::Created)]
    fn should_clone_and_compare_all_variants(#[case] field: SortProductField) {
        let cloned = field;

        assert_eq!(field, cloned);
    }
}
