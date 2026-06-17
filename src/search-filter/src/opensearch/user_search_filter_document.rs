use crate::core::user_search_filter::UserSearchFilter;
use crate::core::user_search_filter::UserSearchFilterSummary;
use crate::core::user_search_filter_name::UserSearchFilterName;
use crate::dynamodb::user_search_filter_record::UserSearchFilterRecord;
use common::actor::document::ActorDocument;
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
    pub notifications: bool,
    #[serde(default)]
    pub state: ResourceStateDocument,
    pub search: ProductSearchDocument,
    pub query: ProductPercolatorQuery,
    pub created_by: ActorDocument,
    pub updated_by: ActorDocument,

    #[serde(with = "time::serde::rfc3339")]
    pub created: OffsetDateTime,

    #[serde(with = "time::serde::rfc3339")]
    pub updated: OffsetDateTime,
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
            search: document.search.into(),
            notifications: document.notifications,
            state: document.state.into(),
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
            notifications: document.notifications,
            state: document.state.into(),
            search: document.search.into(),
            created_by: document.created_by.into(),
            updated_by: document.updated_by.into(),
            created: document.created,
            updated: document.updated,
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
            search: user_search_filter.search.clone().into(),
            query,
            created_by: user_search_filter.created_by.into(),
            updated_by: user_search_filter.updated_by.into(),
            created: user_search_filter.created,
            updated: user_search_filter.updated,
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
