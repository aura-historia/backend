use crate::core::user_search_filter_id::UserSearchFilterId;
use crate::core::user_search_filter_name::UserSearchFilterName;
use crate::dynamodb::user_search_filter_record::UserSearchFilterRecord;
use common::currency::record::CurrencyRecord;
use common::language::record::LanguageRecord;
use common::user_id::UserId;
use product::opensearch::authenticity_document::AuthenticityDocument;
use product::opensearch::condition_document::ConditionDocument;
use product::opensearch::product_state_document::ProductStateDocument;
use product::opensearch::provenance_document::ProvenanceDocument;
use product::opensearch::restoration_document::RestorationDocument;
use serde::{Deserialize, Serialize};
use serde_fields::SerdeField;
use shop::opensearch::shop_type_document::ShopTypeDocument;
use time::OffsetDateTime;

pub type ProductPercolatorQuery = serde_json::Value;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, SerdeField)]
#[serde(rename_all = "camelCase")]
pub struct UserSearchFilterDocument {
    pub user_search_filter_id: UserSearchFilterId,
    pub user_id: UserId,
    pub name: UserSearchFilterName,
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

impl From<UserSearchFilterRecord> for UserSearchFilterDocument {
    fn from(record: UserSearchFilterRecord) -> Self {
        let query = build_percolator_query(&record);
        UserSearchFilterDocument {
            user_search_filter_id: record.user_search_filter_id,
            user_id: record.user_id,
            name: record.name,
            query,
            created: record.created,
            updated: record.updated,
        }
    }
}

fn build_percolator_query(record: &UserSearchFilterRecord) -> ProductPercolatorQuery {
    let mut must = Vec::new();
    let mut must_not = Vec::new();
    let mut filter = Vec::new();

    // Text search
    if let Some(product_query) = &record.product_query {
        let (title_field, description_field) = match record.language {
            LanguageRecord::De => ("titleDe", "descriptionDe"),
            LanguageRecord::En => ("titleEn", "descriptionEn"),
            LanguageRecord::Fr => ("titleFr", "descriptionFr"),
            LanguageRecord::Es => ("titleEs", "descriptionEs"),
            LanguageRecord::It => ("titleIt", "descriptionIt"),
        };
        must.push(serde_json::json!({
            "multi_match": {
                "query": product_query.as_ref(),
                "fields": [
                    format!("{title_field}^3"),
                    format!("{description_field}^1"),
                ],
                "fuzziness": "AUTO",
                "minimum_should_match": "70%"
            }
        }));
    }

    // Exclusions
    if !record.exclude_shop_name_query.is_empty() {
        must_not.push(serde_json::json!({
            "terms": {
                "shopName": record.exclude_shop_name_query.iter().map(AsRef::as_ref).collect::<Vec<&str>>()
            }
        }));
    }

    // Price
    let price_field = match record.currency {
        CurrencyRecord::Eur => "priceEur",
        CurrencyRecord::Gbp => "priceGbp",
        CurrencyRecord::Usd => "priceUsd",
        CurrencyRecord::Aud => "priceAud",
        CurrencyRecord::Cad => "priceCad",
        CurrencyRecord::Nzd => "priceNzd",
    };

    if let Some(price_query) = record.price_query {
        if let Some(min) = price_query.min {
            filter.push(serde_json::json!({ "range": { price_field: { "gte": min } } }));
        }
        if let Some(max) = price_query.max {
            filter.push(serde_json::json!({ "range": { price_field: { "lte": max } } }));
        }
    }

    // Category
    if !record.category_id.is_empty() {
        filter.push(serde_json::json!({
            "terms": { "categoryId": record.category_id.iter().collect::<Vec<_>>() }
        }));
    }

    // Period
    if !record.period_id.is_empty() {
        filter.push(serde_json::json!({
            "terms": { "periodId": record.period_id.iter().collect::<Vec<_>>() }
        }));
    }

    // Origin year (overlap semantics)
    if let Some(origin_query) = record.origin_year_query {
        let mut should = Vec::new();
        match (origin_query.min, origin_query.max) {
            (None, None) => {}
            (Some(qmin), Some(qmax)) if qmin == qmax => {
                should.push(serde_json::json!({
                    "term": { "originYear": qmin }
                }));
            }
            (qmin, qmax) => {
                let mut origin_must = Vec::new();
                if let Some(qmax) = qmax {
                    origin_must.push(serde_json::json!({
                        "range": { "originYearMin": { "lte": qmax } }
                    }));
                }
                if let Some(qmin) = qmin {
                    origin_must.push(serde_json::json!({
                        "range": { "originYearMax": { "gte": qmin } }
                    }));
                }
                should.push(serde_json::json!({
                    "bool": { "must": origin_must }
                }));
            }
        }
        if !should.is_empty() {
            filter.push(serde_json::json!({
                "bool": {
                    "should": should,
                    "minimum_should_match": 1
                }
            }));
        }
    }

    // Shop name
    if !record.shop_name_query.is_empty() {
        filter.push(serde_json::json!({
            "terms": { "shopName": record.shop_name_query.iter().map(AsRef::as_ref).collect::<Vec<&str>>() }
        }));
    }

    // State
    if !record.state_query.is_empty() {
        let values: Vec<&str> = record
            .state_query
            .iter()
            .map(|v| ProductStateDocument::from(*v).as_str())
            .collect();
        filter.push(serde_json::json!({ "terms": { "state": values } }));
    }

    // Authenticity
    if !record.authenticity_query.is_empty() {
        let values: Vec<&str> = record
            .authenticity_query
            .iter()
            .map(|v| AuthenticityDocument::from(*v).as_str())
            .collect();
        filter.push(serde_json::json!({ "terms": { "authenticity": values } }));
    }

    // Condition
    if !record.condition_query.is_empty() {
        let values: Vec<&str> = record
            .condition_query
            .iter()
            .map(|v| ConditionDocument::from(*v).as_str())
            .collect();
        filter.push(serde_json::json!({ "terms": { "condition": values } }));
    }

    // Provenance
    if !record.provenance_query.is_empty() {
        let values: Vec<&str> = record
            .provenance_query
            .iter()
            .map(|v| ProvenanceDocument::from(*v).as_str())
            .collect();
        filter.push(serde_json::json!({ "terms": { "provenance": values } }));
    }

    // Restoration
    if !record.restoration_query.is_empty() {
        let values: Vec<&str> = record
            .restoration_query
            .iter()
            .map(|v| RestorationDocument::from(*v).as_str())
            .collect();
        filter.push(serde_json::json!({ "terms": { "restoration": values } }));
    }

    // Shop type
    if !record.shop_type_query.is_empty() {
        let values: Vec<&str> = record
            .shop_type_query
            .iter()
            .map(|v| ShopTypeDocument::from(*v).as_str())
            .collect();
        filter.push(serde_json::json!({ "terms": { "shopType": values } }));
    }

    // Date range filters
    for (query, field) in [
        (&record.created_query, "created"),
        (&record.updated_query, "updated"),
        (&record.auction_start_query, "auctionStart"),
        (&record.auction_end_query, "auctionEnd"),
    ] {
        if let Some(range_query) = query {
            if let Some(min) = range_query.min {
                let v = min
                    .format(&time::format_description::well_known::Rfc3339)
                    .expect("OffsetDateTime should format as RFC3339");
                filter.push(serde_json::json!({ "range": { field: { "gte": v } } }));
            }
            if let Some(max) = range_query.max {
                let v = max
                    .format(&time::format_description::well_known::Rfc3339)
                    .expect("OffsetDateTime should format as RFC3339");
                filter.push(serde_json::json!({ "range": { field: { "lte": v } } }));
            }
        }
    }

    // Build the bool query, defaulting to match_all if everything is empty
    if must.is_empty() && must_not.is_empty() && filter.is_empty() {
        serde_json::json!({ "match_all": {} })
    } else {
        serde_json::json!({
            "bool": {
                "must": must,
                "must_not": must_not,
                "filter": filter
            }
        })
    }
}

#[cfg(feature = "test-data")]
mod faker {
    use super::*;
    use crate::dynamodb::user_search_filter_record::UserSearchFilterRecord;
    use fake::{Dummy, Fake, Faker, Rng};

    impl Dummy<Faker> for UserSearchFilterDocument {
        fn dummy_with_rng<R: Rng + ?Sized>(config: &Faker, rng: &mut R) -> Self {
            config
                .fake_with_rng::<UserSearchFilterRecord, _>(rng)
                .into()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dynamodb::user_search_filter_record::UserSearchFilterRecord;
    use fake::{Fake, Faker};

    #[test]
    fn should_build_document_from_record_when_empty_filters() {
        let mut record = Faker.fake::<UserSearchFilterRecord>();
        record.product_query = None;
        record.category_id.clear();
        record.period_id.clear();
        record.shop_name_query.clear();
        record.exclude_shop_name_query.clear();
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

        let document: UserSearchFilterDocument = record.into();
        assert_eq!(document.query, serde_json::json!({ "match_all": {} }));
    }

    #[test]
    fn should_build_document_from_record_when_has_category_filter() {
        let mut record = Faker.fake::<UserSearchFilterRecord>();
        let original_categories = record.category_id.clone();
        record.product_query = None;
        record.period_id.clear();
        record.shop_name_query.clear();
        record.exclude_shop_name_query.clear();
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

        if record.category_id.is_empty() {
            return; // Skip if randomly empty
        }

        let document: UserSearchFilterDocument = record.into();
        let query = &document.query;
        assert!(query.get("bool").is_some());
        let filter = &query["bool"]["filter"];
        assert!(filter.is_array());
        let filter_array = filter.as_array().unwrap();
        let has_category_terms = filter_array.iter().any(|f| {
            f.get("terms")
                .is_some_and(|t| t.get("categoryId").is_some())
        });
        if !original_categories.is_empty() {
            assert!(has_category_terms);
        }
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

        let document: UserSearchFilterDocument = record.into();
        assert_eq!(document.user_search_filter_id, expected_id);
        assert_eq!(document.user_id, expected_user_id);
        assert_eq!(document.name, expected_name);
    }
}
