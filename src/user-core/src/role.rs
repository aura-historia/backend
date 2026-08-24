#[cfg_attr(feature = "test-data", derive(::fake::Dummy))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, strum_macros::EnumIter)]
pub enum UserRole {
    #[default]
    User,
    Admin,
}

impl UserRole {
    pub fn from_code(value: &str) -> Option<Self> {
        use strum::IntoEnumIterator;

        Self::iter().find(|role| role.as_str() == value)
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::User => "USER",
            Self::Admin => "ADMIN",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use strum::IntoEnumIterator;

    #[test]
    fn should_default_role_to_user() {
        assert_eq!(UserRole::User, UserRole::default());
    }

    #[test]
    fn should_render_canonical_user_role_identifiers() {
        assert_eq!("USER", UserRole::User.as_str());
        assert_eq!("ADMIN", UserRole::Admin.as_str());
    }

    #[test]
    fn should_have_unique_user_role_identifiers() {
        let identifiers = UserRole::iter()
            .map(UserRole::as_str)
            .collect::<std::collections::HashSet<_>>();

        assert_eq!(UserRole::iter().count(), identifiers.len());
    }

    #[test]
    fn should_round_trip_canonical_user_role_identifiers() {
        for role in UserRole::iter() {
            assert_eq!(Some(role), UserRole::from_code(role.as_str()));
        }
        assert_eq!(None, UserRole::from_code("admin"));
    }
}
