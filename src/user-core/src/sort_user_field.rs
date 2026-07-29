#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SortUserField {
    #[default]
    Name,
    Email,
    FirstName,
    LastName,
    Tier,
    Role,
    Created,
    Updated,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_default_sort_user_field_to_name() {
        assert_eq!(SortUserField::Name, SortUserField::default());
    }
}
