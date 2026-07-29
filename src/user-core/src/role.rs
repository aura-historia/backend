#[cfg_attr(feature = "test-data", derive(::fake::Dummy))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum UserRole {
    #[default]
    User,
    Admin,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_default_role_to_user() {
        assert_eq!(UserRole::User, UserRole::default());
    }
}
