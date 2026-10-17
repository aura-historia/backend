//! Predicates for the supported discovery subset; missing offers never imply prohibition.
use geo::CountryCode;
use money::Price;
use serde_json::{Value, json};
/// Keep the country, service and price constraints inside one nested row.
/// Callers combine this predicate with the ordinary projectionDeleted exclusion.
pub fn country_offer_query(
    country: CountryCode,
    service: Option<&str>,
    maximum_per_item: Option<Price>,
) -> Value {
    let mut filters = vec![json!({"term":{"shipping.countryOffers.country":country.alpha2()}})];
    if let Some(service) = service {
        filters.push(json!({"term":{"shipping.countryOffers.service":service}}));
    }
    if let Some(price) = maximum_per_item {
        filters.push(json!({"term":{"shipping.countryOffers.perItemFixed.currency":price.currency.as_str()}}));
        filters.push(json!({"range":{"shipping.countryOffers.perItemFixed.amountMinor":{"lte":u64::from(price.monetary_amount)}}}));
    }
    json!({"nested":{"path":"shipping.countryOffers","score_mode":"none","query":{"bool":{"filter":filters}}}})
}
