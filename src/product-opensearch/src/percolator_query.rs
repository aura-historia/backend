use crate::product_document::ProductDocumentSerdeField;
use crate::product_search_reader::build_filter_clauses;
use common::language::domain::Language;
use common::query::text_query::TextQuery;
use product_core::product_search::ProductSearch;
use serde_json::json;

/// Builds the stable percolator query for a complete product search.
///
/// This intentionally returns only OpenSearch JSON so other OpenSearch adapters can reuse
/// product matching semantics without receiving product document types.
pub fn build_percolator_query(
    search: &ProductSearch,
) -> Result<serde_json::Value, serde_json::Error> {
    let mut must = Vec::with_capacity(1);
    if let Some(product_query_clause) =
        build_product_query_clause(&search.product_query, title_field(&search.language))
    {
        must.push(product_query_clause);
    }
    let (must_not, filter) = build_filter_clauses(search)?;

    Ok(json!({
        "bool": {
            "must": must,
            "must_not": must_not,
            "filter": filter
        }
    }))
}

fn title_field(language: &Language) -> ProductDocumentSerdeField {
    match language {
        Language::De => ProductDocumentSerdeField::TitleDe,
        Language::En => ProductDocumentSerdeField::TitleEn,
        Language::Fr => ProductDocumentSerdeField::TitleFr,
        Language::Es => ProductDocumentSerdeField::TitleEs,
        Language::It => ProductDocumentSerdeField::TitleIt,
        _ => ProductDocumentSerdeField::TitleEn,
    }
}

fn build_product_query_clause(
    product_queries: &[TextQuery<1>],
    title_field: ProductDocumentSerdeField,
) -> Option<serde_json::Value> {
    match product_queries {
        [] => None,
        [product_query] => Some(build_text_match_clause(product_query.as_ref(), title_field)),
        product_queries => Some(json!({
            "bool": {
                "should": product_queries
                    .iter()
                    .map(|product_query| build_text_match_clause(product_query.as_ref(), title_field))
                    .collect::<Vec<_>>(),
                "minimum_should_match": 1
            }
        })),
    }
}

fn build_text_match_clause(
    product_query: &str,
    title_field: ProductDocumentSerdeField,
) -> serde_json::Value {
    json!({
        "multi_match": {
            "query": product_query,
            "fields": [title_field.as_str(), "title.text"],
            "type": "best_fields",
            "operator": "or",
            "minimum_should_match": "4<80%"
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use common::currency::domain::Currency;
    use common::distance::domain::{Distance, DistanceUnit, GeoDistanceQuery};
    use common::price::domain::MonetaryAmount;
    use common::product_id::ProductId;
    use common::product_lifecycle::domain::ProductLifecycle;
    use common::product_state::domain::ProductState;
    use common::query::range_query::RangeQuery;
    use common::seller_slug_id::SellerSlugId;
    use common::shop_name::ShopName;
    use common::shop_slug_id::ShopSlugId;
    use geo::core::continent::Continent;
    use isocountry::CountryCode;
    use shop_core::shop_type::ShopType;
    use std::collections::HashSet;
    use time::macros::datetime;

    fn text_query(value: &str) -> Result<TextQuery<1>, Box<dyn std::error::Error>> {
        Ok(value.try_into()?)
    }

    #[test]
    fn should_build_full_legacy_percolator_query_with_or_product_queries()
    -> Result<(), Box<dyn std::error::Error>> {
        let excluded_product_id = ProductId::new();
        let excluded_shop_slug_id = ShopSlugId::from("bad-shop");
        let excluded_seller_slug_id = SellerSlugId::from("bad-seller");
        let shop_slug_id = ShopSlugId::from("shop");
        let seller_slug_id = SellerSlugId::from("seller");
        let search = ProductSearch::new(Language::En, Currency::Eur)
            .with_product_query(text_query("Madonna oil painting")?)
            .with_product_query(text_query("Virgin Mary oil painting")?)
            .with_exclude_product_id_query(HashSet::from([excluded_product_id]).into())
            .with_exclude_shop_name_query(HashSet::from([ShopName::from("Bad Shop")]).into())
            .with_exclude_seller_name_query(HashSet::from([ShopName::from("Bad Seller")]).into())
            .with_exclude_shop_slug_id_query(HashSet::from([excluded_shop_slug_id.clone()]).into())
            .with_exclude_seller_slug_id_query(
                HashSet::from([excluded_seller_slug_id.clone()]).into(),
            )
            .with_price_query(RangeQuery {
                min: Some(MonetaryAmount::from(100_u64)),
                max: Some(MonetaryAmount::from(999_u64)),
            })
            .with_country_query(HashSet::from([CountryCode::DEU]).into())
            .with_continent_query(HashSet::from([Continent::Europe]).into())
            .with_geo_address_distance_query(GeoDistanceQuery {
                lat: 52.52,
                lon: 13.405,
                distance: Distance {
                    amount: 10.0,
                    unit: DistanceUnit::Kilometers,
                },
            })
            .with_shop_name_query(HashSet::from([ShopName::from("Shop")]).into())
            .with_seller_name_query(HashSet::from([ShopName::from("Seller")]).into())
            .with_shop_slug_id_query(HashSet::from([shop_slug_id.clone()]).into())
            .with_seller_slug_id_query(HashSet::from([seller_slug_id.clone()]).into())
            .with_state_query(HashSet::from([ProductState::Available]).into())
            .with_shop_type_query(HashSet::from([ShopType::CommercialDealer]).into())
            .with_lifecycle_query(HashSet::from([ProductLifecycle::Deleted]).into())
            .with_created_query(RangeQuery {
                min: Some(datetime!(2025-01-01 0:00 UTC)),
                max: Some(datetime!(2025-01-02 0:00 UTC)),
            })
            .with_updated_query(RangeQuery {
                min: Some(datetime!(2025-01-03 0:00 UTC)),
                max: Some(datetime!(2025-01-04 0:00 UTC)),
            })
            .with_auction_start_query(RangeQuery {
                min: Some(datetime!(2025-01-05 0:00 UTC)),
                max: Some(datetime!(2025-01-06 0:00 UTC)),
            })
            .with_auction_end_query(RangeQuery {
                min: Some(datetime!(2025-01-07 0:00 UTC)),
                max: Some(datetime!(2025-01-08 0:00 UTC)),
            });

        let actual = build_percolator_query(&search)?;

        assert_eq!(
            actual.pointer("/bool/must/0/bool/minimum_should_match"),
            Some(&json!(1))
        );
        assert_eq!(
            actual.pointer("/bool/must/0/bool/should/0/multi_match/query"),
            Some(&json!("Madonna oil painting"))
        );
        assert_eq!(
            actual.pointer("/bool/must/0/bool/should/1/multi_match/query"),
            Some(&json!("Virgin Mary oil painting"))
        );
        assert_eq!(
            actual.pointer("/bool/must/0/bool/should/0/multi_match/fields"),
            Some(&json!(["titleEn", "title.text"]))
        );
        assert_eq!(
            actual.pointer("/bool/must/0/bool/should/0/multi_match/operator"),
            Some(&json!("or"))
        );
        assert_eq!(
            actual.pointer("/bool/must/0/bool/should/0/multi_match/minimum_should_match"),
            Some(&json!("4<80%"))
        );
        assert_eq!(
            actual.pointer("/bool/must_not/0/terms/productId"),
            Some(&json!([excluded_product_id.to_string()]))
        );
        assert_eq!(
            actual.pointer("/bool/must_not/3/terms/shopSlugId"),
            Some(&json!([excluded_shop_slug_id.to_string()]))
        );
        assert_eq!(
            actual.pointer("/bool/must_not/4/terms/sellerSlugId"),
            Some(&json!([excluded_seller_slug_id.to_string()]))
        );
        assert_eq!(
            actual.pointer("/bool/filter/0/terms/lifecycle"),
            Some(&json!(["DELETED"]))
        );
        assert_eq!(
            actual.pointer("/bool/filter/1/range/priceEur/gte"),
            Some(&json!(100))
        );
        assert_eq!(
            actual.pointer("/bool/filter/2/range/priceEur/lte"),
            Some(&json!(999))
        );
        assert_eq!(
            actual.pointer("/bool/filter/3/terms/structuredAddressCountry"),
            Some(&json!(["DE"]))
        );
        assert_eq!(
            actual.pointer("/bool/filter/4/terms/structuredAddressContinent"),
            Some(&json!(["EUROPE"]))
        );
        assert_eq!(
            actual.pointer("/bool/filter/5/geo_distance/distance"),
            Some(&json!("10km"))
        );
        assert_eq!(
            actual.pointer("/bool/filter/5/geo_distance/geoAddress"),
            Some(&json!({"lat": 52.52, "lon": 13.405}))
        );
        assert_eq!(
            actual.pointer("/bool/filter/8/terms/shopSlugId"),
            Some(&json!([shop_slug_id.to_string()]))
        );
        assert_eq!(
            actual.pointer("/bool/filter/9/terms/sellerSlugId"),
            Some(&json!([seller_slug_id.to_string()]))
        );
        assert_eq!(
            actual.pointer("/bool/filter/10/terms/state"),
            Some(&json!(["AVAILABLE"]))
        );
        assert_eq!(
            actual.pointer("/bool/filter/11/terms/shopType"),
            Some(&json!(["COMMERCIAL_DEALER"]))
        );
        assert_eq!(
            actual.pointer("/bool/filter/12/range/created/gte"),
            Some(&json!("2025-01-01T00:00:00Z"))
        );
        assert_eq!(
            actual.pointer("/bool/filter/13/range/created/lte"),
            Some(&json!("2025-01-02T00:00:00Z"))
        );
        assert_eq!(
            actual.pointer("/bool/filter/16/range/auctionStart/gte"),
            Some(&json!("2025-01-05T00:00:00Z"))
        );
        assert_eq!(
            actual.pointer("/bool/filter/19/range/auctionEnd/lte"),
            Some(&json!("2025-01-08T00:00:00Z"))
        );
        Ok(())
    }

    #[test]
    fn should_restrict_percolator_query_to_active_lifecycle_by_default()
    -> Result<(), serde_json::Error> {
        let actual = build_percolator_query(&ProductSearch::new(Language::En, Currency::Eur))?;

        assert_eq!(
            actual.pointer("/bool/filter/0/terms/lifecycle"),
            Some(&json!(["ACTIVE"]))
        );
        Ok(())
    }
}
