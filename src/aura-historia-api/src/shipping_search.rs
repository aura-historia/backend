#![cfg(test)]
pub(crate) use aura_historia_contracts::shipping_search::ShippingSearchData;
use product_listing_core::shipping_search::ShippingSearch;

pub(crate) fn parse(
    d: Option<ShippingSearchData>,
    body: bool,
) -> Result<Option<ShippingSearch>, crate::error::ApiError> {
    aura_historia_contracts::shipping_search::parse(d).map_err(|detail| {
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

#[cfg(test)]
use aura_historia_contracts::saved_searches::AcceptShippingDestinationData;
#[cfg(test)]
mod tests {
    use super::*;
    use geo::reference::ReferenceRelease;
    use money::{Currency, Price};
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
