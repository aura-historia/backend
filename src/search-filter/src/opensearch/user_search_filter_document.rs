use crate::core::user_search_filter::EnhancedSearchDescription;
use crate::core::user_search_filter::UserSearchFilter;
use crate::core::user_search_filter::UserSearchFilterSummary;
use crate::core::user_search_filter_name::UserSearchFilterName;
use crate::dynamodb::user_search_filter_record::UserSearchFilterRecord;
use common::actor::data::ActorData;
use common::resource_state::document::ResourceStateDocument;
use common::user_id::UserId;
use common::user_search_filter_id::UserSearchFilterId;
use product::opensearch::product_search_document::ProductSearchDocument;
use serde::{Deserialize, Serialize};
use serde_fields::SerdeField;
use time::OffsetDateTime;

pub type ProductPercolatorQuery = serde_json::Value;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, SerdeField)]
#[serde(rename_all = "camelCase")]
pub struct UserSearchFilterDocument {
    pub user_search_filter_id: UserSearchFilterId,
    pub user_id: UserId,
    pub name: UserSearchFilterName,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enhanced_search_description: Option<String>,
    pub notifications: bool,
    #[serde(default)]
    pub state: ResourceStateDocument,
    pub search: ProductSearchDocument,
    pub query: ProductPercolatorQuery,
    pub created_by: ActorData,
    pub updated_by: ActorData,

    #[serde(with = "time::serde::rfc3339")]
    pub created: OffsetDateTime,

    #[serde(with = "time::serde::rfc3339")]
    pub updated: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub last_hybrid_search_matched: OffsetDateTime,
}

impl UserSearchFilterDocument {
    pub fn _id(&self) -> UserSearchFilterId {
        self.user_search_filter_id
    }
}

impl From<UserSearchFilterDocument> for UserSearchFilterSummary {
    fn from(document: UserSearchFilterDocument) -> Self {
        UserSearchFilterSummary {
            user_search_filter_id: document.user_search_filter_id,
            user_id: document.user_id,
            name: document.name,
            enhanced_search_description: document
                .enhanced_search_description
                .map(EnhancedSearchDescription::from),
            notifications: document.notifications,
            state: document.state.into(),
            last_hybrid_search_matched: document.last_hybrid_search_matched,
            created_by: document.created_by.into(),
            updated_by: document.updated_by.into(),
            created: document.created,
            updated: document.updated,
        }
    }
}

impl From<UserSearchFilterDocument> for UserSearchFilter {
    fn from(document: UserSearchFilterDocument) -> Self {
        UserSearchFilter {
            user_search_filter_id: document.user_search_filter_id,
            user_id: document.user_id,
            name: document.name,
            enhanced_search_description: document
                .enhanced_search_description
                .map(EnhancedSearchDescription::from),
            notifications: document.notifications,
            state: document.state.into(),
            search: document.search.into(),
            created_by: document.created_by.into(),
            updated_by: document.updated_by.into(),
            created: document.created,
            updated: document.updated,
            last_hybrid_search_matched: document.last_hybrid_search_matched,
        }
    }
}

impl TryFrom<UserSearchFilterRecord> for UserSearchFilterDocument {
    type Error = serde_json::Error;
    fn try_from(user_search_filter_record: UserSearchFilterRecord) -> Result<Self, Self::Error> {
        let user_search_filter = UserSearchFilter::from(user_search_filter_record);
        let query =
            product::opensearch::repository::build_search_query(&user_search_filter.search)?;
        let user_search_filter_doc = UserSearchFilterDocument {
            user_search_filter_id: user_search_filter.user_search_filter_id,
            user_id: user_search_filter.user_id,
            name: user_search_filter.name,
            enhanced_search_description: user_search_filter
                .enhanced_search_description
                .map(Into::into),
            notifications: user_search_filter.notifications,
            state: user_search_filter.state.into(),
            search: user_search_filter.search.clone().into(),
            query,
            created_by: user_search_filter.created_by.into(),
            updated_by: user_search_filter.updated_by.into(),
            created: user_search_filter.created,
            updated: user_search_filter.updated,
            last_hybrid_search_matched: user_search_filter.last_hybrid_search_matched,
        };
        Ok(user_search_filter_doc)
    }
}

#[cfg(feature = "test-data")]
mod faker {
    use super::*;
    use crate::dynamodb::user_search_filter_record::UserSearchFilterRecord;
    use fake::{Dummy, Fake, Faker, RngExt};

    impl Dummy<Faker> for UserSearchFilterDocument {
        fn dummy_with_rng<R: RngExt + ?Sized>(config: &Faker, rng: &mut R) -> Self {
            config
                .fake_with_rng::<UserSearchFilterRecord, _>(rng)
                .try_into()
                .unwrap()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dynamodb::user_search_filter_record::UserSearchFilterRecord;
    use fake::{Fake, Faker};

    #[test]
    fn should_fake_user_search_filter_document() {
        let _ = Faker.fake::<UserSearchFilterDocument>();
    }

    #[test]
    fn should_preserve_metadata_when_converting_from_record() {
        let record = Faker.fake::<UserSearchFilterRecord>();
        let expected_id = record.user_search_filter_id;
        let expected_user_id = record.user_id;
        let expected_name = record.name.clone();

        let document: UserSearchFilterDocument = record.try_into().unwrap();
        assert_eq!(document.user_search_filter_id, expected_id);
        assert_eq!(document.user_id, expected_user_id);
        assert_eq!(document.name, expected_name);
    }
}
