use application::error::{box_error, static_error};
use geo::{country_from_code, reference::ReferenceRelease};
use money::{Currency, Price};
use product_listing_service::shipping_projection::{
    CountryShippingOffer, EffectiveListingShipping, MAX_COUNTRY_OFFERS, POLICY,
    ShippingProjectionError,
};
use serde::{Deserialize, Serialize};
use shipping_core::terms::{ShippingEligibility, ShippingServiceCode};

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Summary {
    schema: String,
    #[serde(deserialize_with = "required")]
    collection: RequiredNullable<String>,
    prohibited: bool,
    conditional: bool,
    country_offers: Vec<Offer>,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Offer {
    country: String,
    service: String,
    #[serde(deserialize_with = "required")]
    per_item_fixed: RequiredNullable<Amount>,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Amount {
    currency: String,
    amount_minor: u64,
}
// Keep explicit null distinct from an absent schema field, as in the terms codec.
#[derive(Serialize)]
#[serde(transparent)]
struct RequiredNullable<T>(Option<T>);

fn required<'de, D, T>(d: D) -> Result<RequiredNullable<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::deserialize(d).map(RequiredNullable)
}
fn invalid() -> ShippingProjectionError {
    ShippingProjectionError::Storage(static_error("invalid persisted effective shipping summary"))
}
fn storage(e: impl std::error::Error + Send + Sync + 'static) -> ShippingProjectionError {
    ShippingProjectionError::Storage(box_error(e))
}
pub(crate) fn encode(
    s: &EffectiveListingShipping,
) -> Result<serde_json::Value, ShippingProjectionError> {
    serde_json::to_value(Summary {
        schema: POLICY.into(),
        collection: RequiredNullable(s.collection.map(|e| e.as_str().into())),
        prohibited: s.prohibited,
        conditional: s.conditional,
        country_offers: s
            .country_offers
            .iter()
            .map(|o| Offer {
                country: o.country.alpha2().into(),
                service: o.service.as_str().into(),
                per_item_fixed: RequiredNullable(o.per_item_fixed.map(|p| Amount {
                    currency: p.currency.as_str().into(),
                    amount_minor: p.monetary_amount.into(),
                })),
            })
            .collect(),
    })
    .map_err(storage)
}
pub(crate) fn decode(
    value: serde_json::Value,
) -> Result<EffectiveListingShipping, ShippingProjectionError> {
    let data: Summary = serde_json::from_value(value).map_err(storage)?;
    if data.schema != POLICY
        || data.country_offers.len() > MAX_COUNTRY_OFFERS
        || (data.prohibited && !data.country_offers.is_empty())
        || (!data.conditional
            && data
                .country_offers
                .iter()
                .any(|offer| offer.per_item_fixed.0.is_none()))
    {
        return Err(invalid());
    }
    let collection = data
        .collection
        .0
        .as_deref()
        .map(|s| ShippingEligibility::from_code(s).ok_or_else(invalid))
        .transpose()?;
    let mut country_offers = Vec::new();
    for offer in data.country_offers {
        let country =
            country_from_code(&offer.country, ReferenceRelease::CURRENT).map_err(storage)?;
        let service = ShippingServiceCode::new(offer.service).map_err(storage)?;
        if country_offers
            .iter()
            .any(|o: &CountryShippingOffer| o.country == country && o.service == service)
        {
            return Err(invalid());
        }
        let per_item_fixed = offer
            .per_item_fixed
            .0
            .map(|p| {
                Ok::<_, ShippingProjectionError>(Price::new(
                    p.amount_minor.into(),
                    Currency::from_code(&p.currency).ok_or_else(invalid)?,
                ))
            })
            .transpose()?;
        country_offers.push(CountryShippingOffer {
            country,
            service,
            per_item_fixed,
        });
    }
    Ok(EffectiveListingShipping {
        collection,
        prohibited: data.prohibited,
        conditional: data.conditional,
        country_offers,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn literal_projection_round_trips_and_invalid_storage_fails_closed() {
        let raw = include_str!("../tests/fixtures/shipping/painting-projection-8000.json");
        let value: serde_json::Value = serde_json::from_str(raw).unwrap();
        assert_eq!(value, encode(&decode(value.clone()).unwrap()).unwrap());
        for poisoned in [
            {
                let mut v = value.clone();
                v["schema"] = "UNKNOWN".into();
                v
            },
            {
                let mut v = value.clone();
                v["prohibited"] = true.into();
                v
            },
            {
                let mut v = value.clone();
                v["country_offers"][0]["country"] = "de".into();
                v
            },
            {
                let mut v = value.clone();
                v["country_offers"][0]["service"] = " ".into();
                v
            },
            {
                let mut v = value.clone();
                v["country_offers"][0]["per_item_fixed"]["currency"] = "eur".into();
                v
            },
            {
                let mut v = value.clone();
                v.as_object_mut().unwrap().remove("collection");
                v
            },
            {
                let mut v = value.clone();
                v["country_offers"][0]
                    .as_object_mut()
                    .unwrap()
                    .remove("per_item_fixed");
                v
            },
            {
                let mut v = value.clone();
                v["conditional"] = false.into();
                v["country_offers"][0]["per_item_fixed"] = serde_json::Value::Null;
                v
            },
            {
                let mut v = value.clone();
                let duplicate = v["country_offers"][0].clone();
                v["country_offers"].as_array_mut().unwrap().push(duplicate);
                v
            },
            {
                let mut v = value.clone();
                v["privateEvidence"] = "forbidden".into();
                v
            },
        ] {
            assert!(decode(poisoned).is_err());
        }
        let mut conditional = value;
        conditional["country_offers"][0]["per_item_fixed"] = serde_json::Value::Null;
        assert!(decode(conditional).unwrap().conditional);
    }
}
