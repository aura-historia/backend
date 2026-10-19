//! Shipping search wire values. Destination predicates never select a browsing preference.
use geo::{country_from_code, reference::ReferenceRelease};
use money::{Currency, Price};
use product_listing_core::shipping_search::ShippingSearch;
use serde::{Deserialize, Serialize};
use shipping_core::terms::ShippingServiceCode;

#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct ShippingSearchData {
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
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "SCREAMING_SNAKE_CASE", deny_unknown_fields)]
enum DestinationData {
    None {},
    Country { country: String },
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct AmountData {
    amount: u64,
    #[serde(with = "crate::wire::currency")]
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
pub(crate) fn parse(
    d: Option<ShippingSearchData>,
    body: bool,
) -> Result<Option<ShippingSearch>, crate::error::ApiError> {
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
    d.map(TryInto::try_into).transpose().map_err(|detail| {
        let error = crate::error::ApiError::bad_request(if body {
            crate::error::BAD_BODY_VALUE
        } else {
            crate::error::BAD_QUERY_PARAMETER_VALUE
        });
        (if body {
            error.with_body_field("search.shipping")
        } else {
            error.with_query_field("shipping")
        })
        .with_detail(detail)
    })
}

/// Optional acceptance guard. The submitted country must equal the authorized address or
/// preference's asserted country. Neither the reference nor provider evidence is retained.
#[derive(Clone, Deserialize)]
#[serde(tag = "kind", rename_all = "SCREAMING_SNAKE_CASE", deny_unknown_fields)]
pub(crate) enum AcceptShippingDestinationData {
    Address {
        #[serde(rename = "addressId")]
        address_id: user_core::user_address::UserAddressId,
    },
    Preference {},
}
impl std::fmt::Debug for AcceptShippingDestinationData {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("AcceptShippingDestinationData(<redacted>)")
    }
}
impl From<AcceptShippingDestinationData>
    for search_filter_service::shipping_destination::AcceptShippingDestination
{
    fn from(d: AcceptShippingDestinationData) -> Self {
        match d {
            AcceptShippingDestinationData::Address { address_id } => Self::Address(address_id),
            AcceptShippingDestinationData::Preference {} => Self::Preference,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn query_and_saved_filter_preserve_source_currency_and_explicit_none() {
        let query: ShippingSearchData = crate::wire::from_query_str("destination[kind]=COUNTRY&destination[country]=DE&maximumPerItem[amount]=9000&maximumPerItem[currency]=EUR&collectionOffered=true").unwrap();
        assert_eq!(
            "ShippingSearchData(<private predicates>)",
            format!("{query:?}")
        );
        let q = parse(Some(query), false).unwrap().unwrap();
        assert_eq!(Some(geo::CountryCode::DEU), q.country());
        assert_eq!(
            Some(Price::new(9000_u64.into(), Currency::Eur)),
            q.maximum_per_item()
        );
        let saved = serde_json::to_value(ShippingSearchData::from(q.clone())).unwrap();
        assert_eq!(
            ReferenceRelease::CURRENT.as_str(),
            saved["referenceRelease"]
        );
        assert_eq!(ShippingSearch::INTERPRETATION, saved["interpretation"]);
        assert_eq!(
            q,
            parse(Some(serde_json::from_value(saved).unwrap()), true)
                .unwrap()
                .unwrap()
        );
        assert!(parse(None, false).unwrap().is_none());
        assert!(
            parse(
                Some(serde_json::from_value(json!({"destination":{"kind":"NONE"}})).unwrap()),
                false
            )
            .unwrap()
            .is_none()
        );
        assert!(
            parse(
                Some(serde_json::from_value(json!({"collectionOffered":true})).unwrap()),
                true
            )
            .unwrap()
            .unwrap()
            .country()
            .is_none()
        );
    }
    #[test]
    fn invalid_or_unsupported_shipping_inputs_fail_closed() {
        for data in [
            json!({}),
            json!({"destination":{"kind":"COUNTRY","country":"DE"},"interpretation":"WHOLE_COUNTRY_PER_ITEM_FX_V1"}),
            json!({"maximumPerItem":{"amount":9000,"currency":"EUR"}}),
            json!({"destination":{"kind":"COUNTRY","country":"DE"},"maximumPerItem":{"amount":-1,"currency":"EUR"}}),
            json!({"destination":{"kind":"COUNTRY","country":"DE"},"maximumPerItem":{"amount":0,"currency":"eur"}}),
            json!({"destination":{"kind":"COUNTRY","country":"DE"},"maximumPerItem":{"amount":9000}}),
            json!({"destination":{"kind":"COUNTRY","country":"XX"}}),
            json!({"destination":{"kind":"COUNTRY","country":"de"}}),
            json!({"destination":{"kind":"COUNTRY","country":"DE","postalCode":"10115"}}),
            json!({"destination":{"kind":"PREFERENCE"}}),
            json!({"destination":{"kind":"COUNTRY","country":"DE"},"service":""}),
            json!({"destination":{"kind":"COUNTRY","country":"DE"},"interpretation":"FUTURE"}),
            json!({"destination":{"kind":"COUNTRY","country":"DE"},"referenceRelease":"current"}),
            json!({"destination":{"kind":"COUNTRY","country":"DE"},"quantity":1}),
            json!({"destination":{"kind":"NONE","country":"DE"}}),
            json!({"destination":{"kind":"NONE","postalCode":"10115"}}),
            json!({"destination":{"kind":"COUNTRY","country":"DE"},"maximumPerItem":{"amount":9223372036854775808_u64,"currency":"EUR"}}),
        ] {
            assert!(
                serde_json::from_value::<ShippingSearchData>(data.clone())
                    .ok()
                    .and_then(|d| parse(Some(d), true).ok())
                    .is_none(),
                "{data}"
            );
        }
    }

    #[test]
    fn preference_acceptance_rejects_unsupported_private_fields() {
        assert!(
            serde_json::from_value::<AcceptShippingDestinationData>(json!({"kind":"PREFERENCE"}))
                .is_ok()
        );
        for data in [
            json!({"kind":"PREFERENCE","addressId":"private"}),
            json!({"kind":"PREFERENCE","postalCode":"10115"}),
        ] {
            assert!(serde_json::from_value::<AcceptShippingDestinationData>(data).is_err());
        }
    }
    #[test]
    fn fx_mode_round_trips_its_explicit_target_independently_of_browsing_currency() {
        let data: ShippingSearchData = crate::wire::from_query_str("destination[kind]=COUNTRY&destination[country]=DE&interpretation=WHOLE_COUNTRY_PER_ITEM_FX_V1&maximumPerItem[amount]=75&maximumPerItem[currency]=JPY").unwrap();
        let query = parse(Some(data), false).unwrap().unwrap();
        assert!(query.fx_converted());
        assert_eq!(Currency::Jpy, query.maximum_per_item().unwrap().currency);
        let saved = serde_json::to_value(ShippingSearchData::from(query.clone())).unwrap();
        assert_eq!(ShippingSearch::FX_INTERPRETATION, saved["interpretation"]);
        assert_eq!(
            query,
            parse(Some(serde_json::from_value(saved).unwrap()), true)
                .unwrap()
                .unwrap()
        );
    }
}
