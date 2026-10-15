//! Discovery predicates over the supported summary, never buyer assessment.
use geo::CountryCode;
use money::Currency;
use serde_json::{Value, json};
/// Every condition belongs to the SAME country/service row. A missing offer proves no prohibition.
pub fn country_offer_query(country: CountryCode, maximum: Option<(Currency, u64)>) -> Value {
    let mut filters = vec![json!({"term":{"shipping.countryOffers.country":country.alpha2()}})];
    if let Some((currency, amount)) = maximum {
        filters.push(
            json!({"term":{"shipping.countryOffers.perItemPrice.currency":currency.as_str()}}),
        );
        filters
            .push(json!({"range":{"shipping.countryOffers.perItemPrice.amount":{"lte":amount}}}));
    }
    json!({"nested":{"path":"shipping.countryOffers","query":{"bool":{"filter":filters}},"score_mode":"none"}})
}
