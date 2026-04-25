use crate::{core::tier::UserTier, dynamodb::tier_record::UserTierRecord};
use serde::{Deserialize, Serialize};

#[cfg_attr(feature = "test-data", derive(::fake::Dummy))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum UserTierDocument {
    Free,
    Pro,
    Ultimate,
}

impl From<UserTierRecord> for UserTierDocument {
    fn from(record: UserTierRecord) -> Self {
        match record {
            UserTierRecord::Free => UserTierDocument::Free,
            UserTierRecord::Pro => UserTierDocument::Pro,
            UserTierRecord::Ultimate => UserTierDocument::Ultimate,
        }
    }
}

impl From<UserTierDocument> for UserTier {
    fn from(doc: UserTierDocument) -> Self {
        match doc {
            UserTierDocument::Free => UserTier::Free,
            UserTierDocument::Pro => UserTier::Pro,
            UserTierDocument::Ultimate => UserTier::Ultimate,
        }
    }
}

impl From<UserTier> for UserTierDocument {
    fn from(value: UserTier) -> Self {
        match value {
            UserTier::Free => UserTierDocument::Free,
            UserTier::Pro => UserTierDocument::Pro,
            UserTier::Ultimate => UserTierDocument::Ultimate,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::UserTierDocument;
    use rstest::rstest;

    #[rstest]
    #[trace]
    #[case(UserTierDocument::Free, "\"FREE\"")]
    #[case(UserTierDocument::Pro, "\"PRO\"")]
    #[case(UserTierDocument::Ultimate, "\"ULTIMATE\"")]
    fn should_serialize_user_tier_document_in_screaming_snake_case(
        #[case] tier: UserTierDocument,
        #[case] expected: &str,
    ) {
        let actual = serde_json::to_string(&tier).unwrap();
        assert_eq!(actual, expected);
    }

    #[rstest]
    #[trace]
    #[case("\"FREE\"", UserTierDocument::Free)]
    #[case("\"PRO\"", UserTierDocument::Pro)]
    #[case("\"ULTIMATE\"", UserTierDocument::Ultimate)]
    fn should_deserialize_user_tier_document_in_screaming_snake_case(
        #[case] tier: &str,
        #[case] expected: UserTierDocument,
    ) {
        let actual = serde_json::from_str::<UserTierDocument>(tier).unwrap();
        assert_eq!(actual, expected);
    }
}
