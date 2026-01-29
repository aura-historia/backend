use common::query::range_query::RangeQuery;
use common::query::text_query::TextQuery;
use common::shop_name::ShopName;
use common::year::Year;
use common::{
    currency::{data::CurrencyData, domain::Currency},
    language::{data::LanguageData, domain::Language},
    price::domain::MonetaryAmount,
    product_state::domain::ProductState,
};
use product::core::authenticity::Authenticity;
use product::core::condition::Condition;
use product::core::provenance::Provenance;
use product::core::restoration::Restoration;
use product::data::authenticity_data::AuthenticityData;
use product::data::condition_data::ConditionData;
use product::data::product_state_data::ProductStateData;
use product::data::provenance_data::ProvenanceData;
use product::data::restoration_data::RestorationData;
use search_filter::core::user_search_filter_name::UserSearchFilterName;
use search_filter::service::user_search_filter_update::UserSearchFilterUpdate;
use serde::{Deserialize, Serialize};
use shop::core::shop_type::ShopType;
use shop::data::shop_type_data::ShopTypeData;
use std::collections::HashSet;
use time::OffsetDateTime;

#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct PatchUserSearchFilterData {
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub name: Option<UserSearchFilterName>,

    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub search: Option<PatchProductSearchData>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct PatchProductSearchData {
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub language: Option<LanguageData>,

    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub currency: Option<CurrencyData>,

    #[serde(
        rename = "productQuery",
        skip_serializing_if = "Option::is_none",
        default
    )]
    pub product_query: Option<TextQuery<3>>,

    #[serde(rename = "shopName", skip_serializing_if = "Option::is_none", default)]
    pub shop_name_query: Option<HashSet<ShopName>>,

    #[serde(rename = "shopType", skip_serializing_if = "Option::is_none", default)]
    pub shop_type_query: Option<HashSet<ShopTypeData>>,

    #[serde(rename = "price", skip_serializing_if = "Option::is_none", default)]
    pub price_query: Option<RangeQuery<u64>>,

    #[serde(rename = "state", skip_serializing_if = "Option::is_none", default)]
    pub state_query: Option<HashSet<ProductStateData>>,

    #[serde(
        rename = "originYear",
        skip_serializing_if = "Option::is_none",
        default
    )]
    pub origin_year_query: Option<RangeQuery<Year>>,
    #[serde(
        rename = "authenticity",
        skip_serializing_if = "Option::is_none",
        default
    )]
    pub authenticity_query: Option<HashSet<AuthenticityData>>,
    #[serde(rename = "condition", skip_serializing_if = "Option::is_none", default)]
    pub condition_query: Option<HashSet<ConditionData>>,
    #[serde(
        rename = "provenance",
        skip_serializing_if = "Option::is_none",
        default
    )]
    pub provenance_query: Option<HashSet<ProvenanceData>>,
    #[serde(
        rename = "restoration",
        skip_serializing_if = "Option::is_none",
        default
    )]
    pub restoration_query: Option<HashSet<RestorationData>>,

    #[serde(
        rename = "created",
        with = "common::query::range_query::range_rfc3339::option",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub created_query: Option<RangeQuery<OffsetDateTime>>,

    #[serde(
        rename = "updated",
        with = "common::query::range_query::range_rfc3339::option",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub updated_query: Option<RangeQuery<OffsetDateTime>>,
}

impl From<PatchUserSearchFilterData> for UserSearchFilterUpdate {
    fn from(patch: PatchUserSearchFilterData) -> Self {
        UserSearchFilterUpdate {
            name: patch.name,
            language: patch
                .search
                .as_ref()
                .and_then(|sf| sf.language.map(Language::from)),
            currency: patch
                .search
                .as_ref()
                .and_then(|sf| sf.currency.map(Currency::from)),
            product_query: patch
                .search
                .as_ref()
                .and_then(|sf| sf.product_query.clone()),
            shop_name_query: patch
                .search
                .as_ref()
                .and_then(|sf| sf.shop_name_query.clone()),
            shop_type_query: patch.search.as_ref().and_then(|sf| {
                sf.shop_type_query
                    .clone()
                    .map(|types| types.into_iter().map(ShopType::from).collect())
            }),
            price_query: patch
                .search
                .as_ref()
                .and_then(|sf| sf.price_query.map(|query| query.map(MonetaryAmount::from))),
            state_query: patch.search.as_ref().and_then(|sf| {
                sf.state_query
                    .clone()
                    .map(|states| states.into_iter().map(ProductState::from).collect())
            }),
            origin_year_query: patch.search.as_ref().and_then(|sf| sf.origin_year_query),
            authenticity_query: patch.search.as_ref().and_then(|sf| {
                sf.authenticity_query
                    .clone()
                    .map(|values| values.into_iter().map(Authenticity::from).collect())
            }),
            condition_query: patch.search.as_ref().and_then(|sf| {
                sf.condition_query
                    .clone()
                    .map(|values| values.into_iter().map(Condition::from).collect())
            }),
            provenance_query: patch.search.as_ref().and_then(|sf| {
                sf.provenance_query
                    .clone()
                    .map(|values| values.into_iter().map(Provenance::from).collect())
            }),
            restoration_query: patch.search.as_ref().and_then(|sf| {
                sf.restoration_query
                    .clone()
                    .map(|values| values.into_iter().map(Restoration::from).collect())
            }),
            created_query: patch.search.as_ref().and_then(|sf| sf.created_query),
            updated_query: patch.search.as_ref().and_then(|sf| sf.updated_query),
            updated: OffsetDateTime::now_utc(),
        }
    }
}

#[cfg(feature = "test-data")]
mod faker {
    use super::*;
    use fake::{Dummy, Fake, Faker, Rng};
    use product::core::product_search::faker::fake_range_query_datetime;

    impl Dummy<Faker> for PatchProductSearchData {
        fn dummy_with_rng<R: Rng + ?Sized>(config: &Faker, rng: &mut R) -> Self {
            PatchProductSearchData {
                language: config.fake_with_rng(rng),
                currency: config.fake_with_rng(rng),
                product_query: config.fake_with_rng(rng),
                shop_name_query: config.fake_with_rng(rng),
                shop_type_query: config.fake_with_rng(rng),
                price_query: config.fake_with_rng(rng),
                state_query: config.fake_with_rng(rng),
                origin_year_query: config.fake_with_rng(rng),
                authenticity_query: config.fake_with_rng(rng),
                condition_query: config.fake_with_rng(rng),
                provenance_query: config.fake_with_rng(rng),
                restoration_query: config.fake_with_rng(rng),
                created_query: fake_range_query_datetime(config, rng),
                updated_query: fake_range_query_datetime(config, rng),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::patch_types::{PatchProductSearchData, PatchUserSearchFilterData};
    use common::query::range_query::RangeQuery;
    use common::shop_name::ShopName;
    use common::{currency::data::CurrencyData, language::data::LanguageData};
    use product::data::authenticity_data::AuthenticityData;
    use product::data::condition_data::ConditionData;
    use product::data::product_state_data::ProductStateData;
    use product::data::provenance_data::ProvenanceData;
    use product::data::restoration_data::RestorationData;
    use serde_json::json;
    use std::collections::HashSet;
    use time::macros::datetime;

    #[test]
    fn should_deserialize_search_filter_patch() {
        let json = json!({
            "language": "de",
            "currency": "EUR",
            "productQuery": "Boop",
            "shopName": ["Baap"],
            "price": {
                "min": 37,
                "max": 42
            },
            "state": ["AVAILABLE"],
            "originYear": {
                "min": 1742,
                "max": 1953
            },
            "authenticity": ["ORIGINAL"],
            "condition": ["EXCELLENT"],
            "provenance": ["PARTIAL"],
            "restoration": ["UNKNOWN"],
            "created": {
                "min": "2000-05-04T00:00:00Z",
                "max": "2025-05-04T00:00:00Z",
            },
            "updated": {
                "min": "2000-05-04T00:00:00Z",
                "max": "2025-05-04T00:00:00Z",
            }
        });
        let expected = PatchProductSearchData {
            language: Some(LanguageData::De),
            currency: Some(CurrencyData::Eur),
            product_query: Some("Boop".try_into().unwrap()),
            shop_name_query: Some(HashSet::from_iter([ShopName::from("Baap")])),
            shop_type_query: None,
            price_query: Some(RangeQuery {
                min: Some(37),
                max: Some(42),
            }),
            state_query: Some(HashSet::from_iter([ProductStateData::Available])),
            origin_year_query: Some(RangeQuery {
                min: Some(1742.into()),
                max: Some(1953.into()),
            }),
            authenticity_query: Some(HashSet::from_iter([AuthenticityData::Original])),
            condition_query: Some(HashSet::from_iter([ConditionData::Excellent])),
            provenance_query: Some(HashSet::from_iter([ProvenanceData::Partial])),
            restoration_query: Some(HashSet::from_iter([RestorationData::Unknown])),
            created_query: Some(RangeQuery {
                min: Some(datetime!(2000 - 05 - 04 0:00 UTC)),
                max: Some(datetime!(2025 - 05 - 04 0:00 UTC)),
            }),
            updated_query: Some(RangeQuery {
                min: Some(datetime!(2000 - 05 - 04 0:00 UTC)),
                max: Some(datetime!(2025 - 05 - 04 0:00 UTC)),
            }),
        };

        let actual: PatchProductSearchData = serde_json::from_value(json).unwrap();

        assert_eq!(expected, actual);
    }

    #[test]
    fn should_deserialize_user_search_filter_patch() {
        let json = json!({
            "name": "hugos filter for peppino",
            "search": {
                "language": "de",
                "currency": "EUR",
                "productQuery": "Boop",
                "shopName": ["Baap"],
                "price": {
                    "min": 37,
                    "max": 42
                },
                "state": ["AVAILABLE"],
                "originYear": {
                    "min": 1742,
                    "max": 1953
                },
                "authenticity": ["ORIGINAL"],
                "condition": ["EXCELLENT"],
                "provenance": ["PARTIAL"],
                "restoration": ["UNKNOWN"],
                "created": {
                    "min": "2000-05-04T00:00:00Z",
                    "max": "2025-05-04T00:00:00Z",
                },
                "updated": {
                    "min": "2000-05-04T00:00:00Z",
                    "max": "2025-05-04T00:00:00Z",
                }
            }
        });
        let expected = PatchUserSearchFilterData {
            name: Some("hugos filter for peppino".into()),
            search: Some(PatchProductSearchData {
                language: Some(LanguageData::De),
                currency: Some(CurrencyData::Eur),
                product_query: Some("Boop".try_into().unwrap()),
                shop_name_query: Some(["Baap".into()].into()),
                shop_type_query: None,
                price_query: Some(RangeQuery {
                    min: Some(37),
                    max: Some(42),
                }),
                state_query: Some(HashSet::from_iter([ProductStateData::Available])),
                origin_year_query: Some(RangeQuery {
                    min: Some(1742.into()),
                    max: Some(1953.into()),
                }),
                authenticity_query: Some(HashSet::from_iter([AuthenticityData::Original])),
                condition_query: Some(HashSet::from_iter([ConditionData::Excellent])),
                provenance_query: Some(HashSet::from_iter([ProvenanceData::Partial])),
                restoration_query: Some(HashSet::from_iter([RestorationData::Unknown])),
                created_query: Some(RangeQuery {
                    min: Some(datetime!(2000 - 05 - 04 0:00 UTC)),
                    max: Some(datetime!(2025 - 05 - 04 0:00 UTC)),
                }),
                updated_query: Some(RangeQuery {
                    min: Some(datetime!(2000 - 05 - 04 0:00 UTC)),
                    max: Some(datetime!(2025 - 05 - 04 0:00 UTC)),
                }),
            }),
        };

        let actual: PatchUserSearchFilterData = serde_json::from_value(json).unwrap();

        assert_eq!(expected, actual);
    }
}
