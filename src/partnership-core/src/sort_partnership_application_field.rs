#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SortPartnershipApplicationField {
    #[default]
    Created,
    Updated,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_default_sort_partnership_application_field_to_created() {
        assert_eq!(
            SortPartnershipApplicationField::Created,
            SortPartnershipApplicationField::default()
        );
    }

    #[test]
    fn should_clone_and_compare_each_sort_partnership_application_field() {
        for field in [
            SortPartnershipApplicationField::Created,
            SortPartnershipApplicationField::Updated,
        ] {
            assert_eq!(field, field.clone());
        }
    }
}
