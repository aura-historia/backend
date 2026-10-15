//! Adapter protocol shared by PostgreSQL and search; no private policy fields.
use application::error::{BoxError, static_error};
use geo::{country_from_code, reference::ReferenceRelease};
use money::{Currency, Price};
use serde::{Deserialize, Serialize};
use shipping_core::{
    discovery::{CountryOffer, EffectiveShipping, POLICY, validate},
    terms::ShippingEligibility,
};

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Data {
    schema: String,
    collection: serde_json::Value,
    shipping_prohibited: bool,
    conditional: bool,
    country_offers: Vec<Offer>,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Offer {
    country: String,
    zone: u16,
    service: u16,
    per_item_price: serde_json::Value,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Amount {
    amount: u64,
    currency: String,
}
fn invalid() -> BoxError {
    static_error("invalid effective shipping projection")
}
pub fn encode(summary: &EffectiveShipping) -> Result<serde_json::Value, BoxError> {
    if !validate(summary) {
        return Err(invalid());
    }
    Ok(serde_json::to_value(Data {
        schema: POLICY.into(),
        collection: serde_json::to_value(summary.collection.map(|c| c.as_str()))?,
        shipping_prohibited: summary.shipping_prohibited,
        conditional: summary.conditional,
        country_offers: summary.country_offers.iter().map(|o| Offer {
            country: o.country.alpha2().into(), zone: o.zone, service: o.service,
            per_item_price: o.per_item_price.map(|p| serde_json::json!({"amount":u64::from(p.monetary_amount),"currency":p.currency.as_str()})).unwrap_or(serde_json::Value::Null),
        }).collect(),
    })?)
}
pub fn decode(value: serde_json::Value) -> Result<EffectiveShipping, BoxError> {
    let d: Data = serde_json::from_value(value)?;
    if d.schema != POLICY {
        return Err(invalid());
    }
    let summary = EffectiveShipping {
        collection: if d.collection.is_null() {
            None
        } else {
            Some(
                ShippingEligibility::from_code(d.collection.as_str().ok_or_else(invalid)?)
                    .ok_or_else(invalid)?,
            )
        },
        shipping_prohibited: d.shipping_prohibited,
        conditional: d.conditional,
        country_offers: d
            .country_offers
            .into_iter()
            .map(|o| {
                let price = if o.per_item_price.is_null() {
                    None
                } else {
                    let p: Amount = serde_json::from_value(o.per_item_price)?;
                    Some(Price::new(
                        p.amount.into(),
                        Currency::from_code(&p.currency).ok_or_else(invalid)?,
                    ))
                };
                Ok(CountryOffer {
                    country: country_from_code(&o.country, ReferenceRelease::CURRENT)?,
                    zone: o.zone,
                    service: o.service,
                    per_item_price: price,
                })
            })
            .collect::<Result<_, BoxError>>()?,
    };
    if !validate(&summary) {
        return Err(invalid());
    }
    Ok(summary)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn literal_projection_codec_is_strict_and_preserves_minor_units() {
        let fixtures: serde_json::Value =
            serde_json::from_str(include_str!("../fixtures/gallery-projection.json")).unwrap();
        for key in ["paintingBefore", "paintingAfter", "cabinet"] {
            let value = fixtures[key].clone();
            let summary = decode(value.clone()).unwrap();
            assert_eq!(value, encode(&summary).unwrap());
            for field in [
                "schema",
                "collection",
                "shipping_prohibited",
                "conditional",
                "country_offers",
            ] {
                let mut bad = value.clone();
                bad.as_object_mut().unwrap().remove(field);
                assert!(decode(bad).is_err(), "missing {field}");
            }
            let mut bad = value.clone();
            bad["privateEvidence"] = serde_json::json!("disallowed");
            assert!(decode(bad).is_err());
            let mut bad = value.clone();
            bad["schema"] = serde_json::json!("FUTURE");
            assert!(decode(bad).is_err());
        }
        let mut bad = fixtures["paintingBefore"].clone();
        bad["country_offers"][0]["country"] = serde_json::json!("ZZ");
        assert!(decode(bad).is_err());
        let mut bad = fixtures["paintingBefore"].clone();
        bad["country_offers"][0]["per_item_price"]["currency"] = serde_json::json!("eur");
        assert!(decode(bad).is_err());
    }
}
