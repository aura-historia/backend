//! Predicates for the supported discovery subset; missing offers never imply prohibition.
use geo::CountryCode;
use money::Price;
use product_listing_service::ports::NativePriceRange;
use serde_json::{Value, json};

/// Keep country, service, currency and price constraints inside one nested row.
/// Callers combine this predicate with the ordinary projectionDeleted exclusion.
pub fn country_offer_query(
    country: CountryCode,
    service: Option<&str>,
    maximum_per_item: Option<Price>,
) -> Value {
    let price_filters = maximum_per_item.map(|price| vec![
        json!({"term":{"shipping.countryOffers.perItemFixed.currency":price.currency.as_str()}}),
        json!({"range":{"shipping.countryOffers.perItemFixed.amountMinor":{"lte":u64::from(price.monetary_amount)}}}),
    ]).unwrap_or_default();
    nested_offer(country, service, price_filters)
}

fn nested_offer(country: CountryCode, service: Option<&str>, price_filters: Vec<Value>) -> Value {
    let mut filters = vec![json!({"term":{"shipping.countryOffers.country":country.alpha2()}})];
    if let Some(service) = service {
        filters.push(json!({"term":{"shipping.countryOffers.service":service}}));
    }
    filters.extend(price_filters);
    json!({"nested":{"path":"shipping.countryOffers","score_mode":"none","query":{"bool":{"filter":filters}}}})
}

fn native_range(range: &NativePriceRange) -> Value {
    let mut amount = json!({"gte":range.lower});
    if let Some(upper) = range.upper {
        amount["lte"] = json!(upper.min(i64::MAX as u64));
    }
    json!({"bool":{"filter":[
        {"term":{"shipping.countryOffers.perItemFixed.currency":range.source_currency.as_str()}},
        {"range":{"shipping.countryOffers.perItemFixed.amountMinor":amount}}
    ]}})
}

/// All shipping predicates constrain indexed retrieval, including hybrid retrieval and totals.
/// FX bounds are compiled by the service using the same snapshot as response conversion.
pub(crate) fn clauses(
    query: &product_listing_core::shipping_search::ShippingSearch,
    native_ranges: Option<&[NativePriceRange]>,
) -> Vec<Value> {
    let mut clauses = Vec::new();
    if let Some(country) = query.country() {
        let service = query.service().map(|s| s.as_str());
        clauses.push(if query.fx_converted() {
            let alternatives = native_ranges
                .unwrap_or_default()
                .iter()
                .map(native_range)
                .collect::<Vec<_>>();
            let price_filter = if alternatives.is_empty() {
                json!({"match_none":{}})
            } else {
                json!({"bool":{"should":alternatives,"minimum_should_match":1}})
            };
            nested_offer(country, service, vec![price_filter])
        } else {
            country_offer_query(country, service, query.maximum_per_item())
        });
    }
    if query.collection_offered() {
        clauses.push(json!({"term":{"shipping.collection":"OFFERED"}}));
    }
    clauses
}
