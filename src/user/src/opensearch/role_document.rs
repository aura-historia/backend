use crate::{core::role::UserRole, dynamodb::role_record::UserRoleRecord};
use serde::{Deserialize, Serialize};

#[cfg_attr(feature = "test-data", derive(::fake::Dummy))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum UserRoleDocument {
    User,
    Admin,
}

impl From<UserRoleRecord> for UserRoleDocument {
    fn from(record: UserRoleRecord) -> Self {
        match record {
            UserRoleRecord::User => UserRoleDocument::User,
            UserRoleRecord::Admin => UserRoleDocument::Admin,
        }
    }
}

impl From<UserRoleDocument> for UserRole {
    fn from(doc: UserRoleDocument) -> Self {
        match doc {
            UserRoleDocument::User => UserRole::User,
            UserRoleDocument::Admin => UserRole::Admin,
        }
    }
}

impl From<UserRole> for UserRoleDocument {
    fn from(value: UserRole) -> Self {
        match value {
            UserRole::User => UserRoleDocument::User,
            UserRole::Admin => UserRoleDocument::Admin,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::UserRoleDocument;
    use rstest::rstest;

    #[rstest]
    #[trace]
    #[case(UserRoleDocument::User, "\"USER\"")]
    #[case(UserRoleDocument::Admin, "\"ADMIN\"")]
    fn should_serialize_user_role_document_in_screaming_snake_case(
        #[case] role: UserRoleDocument,
        #[case] expected: &str,
    ) {
        let actual = serde_json::to_string(&role).unwrap();
        assert_eq!(actual, expected);
    }

    #[rstest]
    #[trace]
    #[case("\"USER\"", UserRoleDocument::User)]
    #[case("\"ADMIN\"", UserRoleDocument::Admin)]
    fn should_deserialize_user_role_document_in_screaming_snake_case(
        #[case] role: &str,
        #[case] expected: UserRoleDocument,
    ) {
        let actual = serde_json::from_str::<UserRoleDocument>(role).unwrap();
        assert_eq!(actual, expected);
    }
}
