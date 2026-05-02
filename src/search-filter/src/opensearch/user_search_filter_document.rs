use crate::core::user_search_filter::EnhancedSearchDescription;
use crate::core::user_search_filter::UserSearchFilter;
use crate::core::user_search_filter::UserSearchFilterSummary;
use crate::core::user_search_filter_name::UserSearchFilterName;
use crate::dynamodb::user_search_filter_record::UserSearchFilterRecord;
use crate::opensearch::user_search_filter_state_document::UserSearchFilterStateDocument;
use common::user_id::UserId;
use common::user_search_filter_id::UserSearchFilterId;
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
    pub state: UserSearchFilterStateDocument,
    pub query: ProductPercolatorQuery,

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
            enhanced_search_description: document
                .enhanced_search_description
                .map(EnhancedSearchDescription::from),
            notifications: document.notifications,
            state: document.state.into(),
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
            query,
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
    fn should_build_document_from_record_when_has_category_filter() {
        use common::category_key::CategoryId;
        use std::collections::HashSet;

        let mut record = Faker.fake::<UserSearchFilterRecord>();
        record.product_query = None;
        record.category_id = HashSet::from([CategoryId::from("furniture")]);
        record.period_id.clear();
        record.shop_name_query.clear();
        record.seller_name_query.clear();
        record.exclude_shop_name_query.clear();
        record.exclude_seller_name_query.clear();
        record.shop_type_query.clear();
        record.price_query = None;
        record.state_query.clear();
        record.origin_year_query = None;
        record.authenticity_query.clear();
        record.condition_query.clear();
        record.provenance_query.clear();
        record.restoration_query.clear();
        record.created_query = None;
        record.updated_query = None;
        record.auction_start_query = None;
        record.auction_end_query = None;

        let document: UserSearchFilterDocument = record.try_into().unwrap();
        let query = &document.query;

        // With no product_query, build_search_query wraps in constant_score instead of bool
        assert!(query.get("constant_score").is_some());
        let filter = &query["constant_score"]["filter"]["bool"]["filter"];
        assert!(filter.is_array());
        let filter_array = filter.as_array().unwrap();
        let has_category_terms = filter_array.iter().any(|f| {
            f.get("terms")
                .is_some_and(|t| t.get("categoryId").is_some())
        });
        assert!(has_category_terms);
    }

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
