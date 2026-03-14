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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, SerdeField)]
#[serde(rename_all = "camelCase")]
pub struct UserSearchFilterDocument {
    pub user_search_filter_id: UserSearchFilterId,
    pub user_id: UserId,
    pub name: UserSearchFilterName,
    pub query: serde_json::Value,

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

fn build_percolator_query(record: &UserSearchFilterRecord) -> serde_json::Value {
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
                    .unwrap_or_default();
                filter.push(serde_json::json!({ "range": { field: { "gte": v } } }));
            }
            if let Some(max) = range_query.max {
                let v = max
                    .format(&time::format_description::well_known::Rfc3339)
                    .unwrap_or_default();
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

pub fn build_percolation_document(
    product: &product::core::product::Product,
) -> Result<serde_json::Value, serde_json::Error> {
    use common::language::domain::Language;
    use product::opensearch::authenticity_document::AuthenticityDocument;
    use product::opensearch::condition_document::ConditionDocument;
    use product::opensearch::product_state_document::ProductStateDocument;
    use product::opensearch::provenance_document::ProvenanceDocument;
    use product::opensearch::restoration_document::RestorationDocument;
    use serde::ser::Error;
    use shop::opensearch::shop_type_document::ShopTypeDocument;

    let mut doc = serde_json::json!({
        "shopName": product.shop_name.as_ref(),
        "shopType": ShopTypeDocument::from(product.shop_type).as_str(),
        "state": ProductStateDocument::from(product.state).as_str(),
        "authenticity": AuthenticityDocument::from(product.authenticity).as_str(),
        "condition": ConditionDocument::from(product.condition).as_str(),
        "provenance": ProvenanceDocument::from(product.provenance).as_str(),
        "restoration": RestorationDocument::from(product.restoration).as_str(),
    });

    if let Some(category_id) = &product.category_id {
        doc["categoryId"] = serde_json::json!(category_id);
    }
    if let Some(period_id) = &product.period_id {
        doc["periodId"] = serde_json::json!(period_id);
    }

    // Titles
    for (lang, title) in &product.other_title {
        let field = match lang {
            Language::De => "titleDe",
            Language::En => "titleEn",
            Language::Fr => "titleFr",
            Language::Es => "titleEs",
            Language::It => "titleIt",
        };
        doc[field] = serde_json::json!(title.as_ref());
    }
    let native_title_field = match product.native_title.localization {
        Language::De => "titleDe",
        Language::En => "titleEn",
        Language::Fr => "titleFr",
        Language::Es => "titleEs",
        Language::It => "titleIt",
    };
    doc[native_title_field] = serde_json::json!(product.native_title.payload.as_ref());

    // Descriptions
    if let Some(native_desc) = &product.native_description {
        let field = match native_desc.localization {
            Language::De => "descriptionDe",
            Language::En => "descriptionEn",
            Language::Fr => "descriptionFr",
            Language::Es => "descriptionEs",
            Language::It => "descriptionIt",
        };
        doc[field] = serde_json::json!(native_desc.payload.as_ref());
    }
    for (lang, desc) in &product.other_description {
        let field = match lang {
            Language::De => "descriptionDe",
            Language::En => "descriptionEn",
            Language::Fr => "descriptionFr",
            Language::Es => "descriptionEs",
            Language::It => "descriptionIt",
        };
        doc[field] = serde_json::json!(desc.as_ref());
    }

    // Prices
    if let Some(price) = &product.native_price {
        let field = match price.currency {
            common::currency::domain::Currency::Eur => "priceEur",
            common::currency::domain::Currency::Gbp => "priceGbp",
            common::currency::domain::Currency::Usd => "priceUsd",
            common::currency::domain::Currency::Aud => "priceAud",
            common::currency::domain::Currency::Cad => "priceCad",
            common::currency::domain::Currency::Nzd => "priceNzd",
        };
        doc[field] = serde_json::json!(u64::from(price.monetary_amount));
    }
    for (currency, amount) in &product.other_price {
        let field = match currency {
            common::currency::domain::Currency::Eur => "priceEur",
            common::currency::domain::Currency::Gbp => "priceGbp",
            common::currency::domain::Currency::Usd => "priceUsd",
            common::currency::domain::Currency::Aud => "priceAud",
            common::currency::domain::Currency::Cad => "priceCad",
            common::currency::domain::Currency::Nzd => "priceNzd",
        };
        doc[field] = serde_json::json!(u64::from(*amount));
    }

    // Origin year
    if let Some(origin_year) = &product.origin_year {
        if let Some(exact) = origin_year.exact() {
            doc["originYear"] = serde_json::json!(exact);
        }
        if let Some(min) = origin_year.min() {
            doc["originYearMin"] = serde_json::json!(min);
        }
        if let Some(max) = origin_year.max() {
            doc["originYearMax"] = serde_json::json!(max);
        }
    }

    // Date fields
    if let Some(auction_start) = product.auction_start {
        doc["auctionStart"] = serde_json::json!(auction_start
            .format(&time::format_description::well_known::Rfc3339)
            .map_err(|e| serde_json::Error::custom(e))?);
    }
    if let Some(auction_end) = product.auction_end {
        doc["auctionEnd"] = serde_json::json!(auction_end
            .format(&time::format_description::well_known::Rfc3339)
            .map_err(|e| serde_json::Error::custom(e))?);
    }
    doc["created"] = serde_json::json!(product
        .created
        .format(&time::format_description::well_known::Rfc3339)
        .map_err(|e| serde_json::Error::custom(e))?);
    doc["updated"] = serde_json::json!(product
        .updated
        .format(&time::format_description::well_known::Rfc3339)
        .map_err(|e| serde_json::Error::custom(e))?);

    Ok(doc)
}

#[cfg(feature = "test-data")]
mod faker {
    use super::*;
    use crate::dynamodb::user_search_filter_record::UserSearchFilterRecord;
    use fake::{Dummy, Fake, Faker, Rng};

    impl Dummy<Faker> for UserSearchFilterDocument {
        fn dummy_with_rng<R: Rng + ?Sized>(config: &Faker, rng: &mut R) -> Self {
            config.fake_with_rng::<UserSearchFilterRecord, _>(rng).into()
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
        let has_category_terms = filter_array
            .iter()
            .any(|f| f.get("terms").is_some_and(|t| t.get("categoryId").is_some()));
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
