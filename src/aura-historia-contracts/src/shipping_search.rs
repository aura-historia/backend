//! Shipping search wire values. Destination predicates never select a browsing preference.
use geo::{country_from_code, reference::ReferenceRelease};
use money::{Currency, Price};
use product_listing_core::shipping_search::ShippingSearch;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use shipping_core::terms::ShippingServiceCode;

#[derive(Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ShippingSearchData {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    destination: Option<DestinationData>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    reference_release: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    interpretation: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    service: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    maximum_per_item: Option<AmountData>,
    #[serde(default, skip_serializing_if = "is_false")]
    collection_offered: bool,
}
#[derive(Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "SCREAMING_SNAKE_CASE", deny_unknown_fields)]
enum DestinationData {
    None {},
    Country { country: String },
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct AmountData {
    #[schemars(range(max = i64::MAX))]
    amount: u64,
    #[serde(with = "crate::wire::currency")]
    #[schemars(schema_with = "crate::wire::currency::schema")]
    currency: Currency,
}
impl std::fmt::Debug for ShippingSearchData {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ShippingSearchData(<private predicates>)")
    }
}
impl TryFrom<ShippingSearchData> for ShippingSearch {
    type Error = &'static str;
    fn try_from(d: ShippingSearchData) -> Result<Self, Self::Error> {
        let release = d
            .reference_release
            .as_deref()
            .map(ReferenceRelease::from_code)
            .transpose()
            .map_err(|_| "unsupported shipping reference release")?
            .unwrap_or(ReferenceRelease::CURRENT);
        if d.interpretation.as_deref().is_some_and(|p| {
            p != ShippingSearch::INTERPRETATION && p != ShippingSearch::FX_INTERPRETATION
        }) {
            return Err("unsupported shipping interpretation");
        }
        let country = match d.destination {
            Some(DestinationData::Country { country }) => {
                Some(country_from_code(&country, release).map_err(|_| "invalid shipping country")?)
            }
            _ => None,
        };
        let service = d
            .service
            .map(ShippingServiceCode::new)
            .transpose()
            .map_err(|_| "invalid shipping service")?;
        let query = Self::new(release, country, service, d.maximum_per_item.map(|p| Price::new(p.amount.into(), p.currency)), d.collection_offered)
            .map_err(|_| "shipping requires a country or collection offered; service and per-item maximum require a country")?;
        if d.interpretation.as_deref() == Some(ShippingSearch::FX_INTERPRETATION) {
            query
                .with_fx_conversion()
                .map_err(|_| "FX shipping interpretation requires a maximum")
        } else {
            Ok(query)
        }
    }
}
impl From<ShippingSearch> for ShippingSearchData {
    fn from(q: ShippingSearch) -> Self {
        Self {
            reference_release: Some(q.reference_release().as_str().into()),
            interpretation: Some(q.interpretation().into()),
            destination: q.country().map(|c| DestinationData::Country {
                country: c.alpha2().into(),
            }),
            service: q.service().map(|s| s.as_str().into()),
            maximum_per_item: q.maximum_per_item().map(|p| AmountData {
                amount: p.monetary_amount.into(),
                currency: p.currency,
            }),
            collection_offered: q.collection_offered(),
        }
    }
}
fn is_false(value: &bool) -> bool {
    !value
}
pub fn parse(d: Option<ShippingSearchData>) -> Result<Option<ShippingSearch>, &'static str> {
    if d.as_ref().is_some_and(|d| {
        matches!(d.destination, Some(DestinationData::None {}))
            && d.service.is_none()
            && d.maximum_per_item.is_none()
            && !d.collection_offered
            && d.reference_release
                .as_deref()
                .is_none_or(|r| ReferenceRelease::from_code(r).is_ok())
            && d.interpretation
                .as_deref()
                .is_none_or(|p| p == ShippingSearch::INTERPRETATION)
    }) {
        return Ok(None);
    }
    d.map(TryInto::try_into).transpose()
}
