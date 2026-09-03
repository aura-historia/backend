#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SortPartyField {
    #[default]
    Name,
    Email,
    Phone,
    Created,
    Updated,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_default_sort_party_field_to_name() {
        assert_eq!(SortPartyField::Name, SortPartyField::default());
    }
}
