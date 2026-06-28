use crate::core::user_search_filter::UserSearchFilter;
use crate::core::user_search_filter_name::UserSearchFilterName;
use crate::dynamodb::user_search_filter_record::UserSearchFilterRecord;
use common::actor::document::ActorDocument;
use common::resource_state::document::ResourceStateDocument;
use common::user_id::UserId;
use common::user_search_filter_id::UserSearchFilterId;
use product::core::product_search::EnhancedSearchDescription;
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
    pub notifications: bool,
    #[serde(default)]
    pub state: ResourceStateDocument,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub enhanced_search_description: Option<String>,
    pub search: ProductSearchDocument,
    pub query: ProductPercolatorQuery,
    // dim=768 via google/gemini-embedding-2
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub embedding: Option<Vec<f32>>,
    pub created_by: ActorDocument,
    pub updated_by: ActorDocument,

    #[serde(with = "time::serde::rfc3339")]
    pub created: OffsetDateTime,

    #[serde(with = "time::serde::rfc3339")]
    pub updated: OffsetDateTime,

    #[serde(
        with = "time::serde::rfc3339",
        default = "default_last_hybrid_search_matched"
    )]
    pub last_hybrid_search_matched: OffsetDateTime,
}

fn default_last_hybrid_search_matched() -> OffsetDateTime {
    OffsetDateTime::UNIX_EPOCH
}

impl UserSearchFilterDocument {
    pub fn _id(&self) -> UserSearchFilterId {
        self.user_search_filter_id
    }
}

impl From<UserSearchFilterDocument> for UserSearchFilter {
    fn from(document: UserSearchFilterDocument) -> Self {
        let mut search = product::core::product_search::ProductSearch::from(document.search);
        if search.enhanced_search_description.is_none() {
            search.enhanced_search_description = document
                .enhanced_search_description
                .map(EnhancedSearchDescription::from);
        }

        UserSearchFilter {
            user_search_filter_id: document.user_search_filter_id,
            user_id: document.user_id,
            name: document.name,
            notifications: document.notifications,
            state: document.state.into(),
            search,
            created_by: document.created_by.into(),
            updated_by: document.updated_by.into(),
            created: document.created,
            updated: document.updated,
            last_hybrid_search_matched: document.last_hybrid_search_matched,
            embedding: document.embedding,
        }
    }
}

impl TryFrom<UserSearchFilterRecord> for UserSearchFilterDocument {
    type Error = serde_json::Error;
    fn try_from(user_search_filter_record: UserSearchFilterRecord) -> Result<Self, Self::Error> {
        let user_search_filter = UserSearchFilter::from(user_search_filter_record);
        let query =
            product::opensearch::repository::build_percolator_query(&user_search_filter.search)?;
        let user_search_filter_doc = UserSearchFilterDocument {
            user_search_filter_id: user_search_filter.user_search_filter_id,
            user_id: user_search_filter.user_id,
            name: user_search_filter.name,
            notifications: user_search_filter.notifications,
            state: user_search_filter.state.into(),
            enhanced_search_description: user_search_filter
                .search
                .enhanced_search_description
                .clone()
                .map(Into::into),
            search: user_search_filter.search.clone().into(),
            query,
            embedding: user_search_filter.embedding,
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
