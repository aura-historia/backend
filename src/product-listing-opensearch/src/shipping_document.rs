use serde::{Deserialize, Serialize};
use shipping_core::discovery::{EffectiveShipping, POLICY};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct ShippingDocument {
    policy: String,
    collection: Nullable<String>,
    shipping_prohibited: bool,
    conditional: bool,
    country_offers: Vec<CountryOfferDocument>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CountryOfferDocument {
    country: String,
    zone: u16,
    service: u16,
    per_item_price: Nullable<AmountDocument>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct AmountDocument {
    amount: u64,
    currency: String,
}
/// Nullable values still require an explicit field in the persisted document.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
enum Nullable<T> {
    Value(T),
    Null,
}
impl<T> From<Option<T>> for Nullable<T> {
    fn from(value: Option<T>) -> Self {
        match value {
            Some(value) => Self::Value(value),
            None => Self::Null,
        }
    }
}
impl From<&EffectiveShipping> for ShippingDocument {
    fn from(s: &EffectiveShipping) -> Self {
        Self {
            policy: POLICY.into(),
            collection: s.collection.map(|c| c.as_str().into()).into(),
            shipping_prohibited: s.shipping_prohibited,
            conditional: s.conditional,
            country_offers: s
                .country_offers
                .iter()
                .map(|o| CountryOfferDocument {
                    country: o.country.alpha2().into(),
                    zone: o.zone,
                    service: o.service,
                    per_item_price: o
                        .per_item_price
                        .map(|p| AmountDocument {
                            amount: u64::from(p.monetary_amount),
                            currency: p.currency.as_str().into(),
                        })
                        .into(),
                })
                .collect(),
        }
    }
}
impl ShippingDocument {
    pub(crate) fn validate(&self) -> bool {
        // Map back through the strict adapter codec; malformed stored search fields
        // cannot turn into permissive predicates or silently valid read state.
        shipping_json::discovery::decode(serde_json::json!({"schema":self.policy,"collection":self.collection,"shipping_prohibited":self.shipping_prohibited,"conditional":self.conditional,
            "country_offers":self.country_offers.iter().map(|o|serde_json::json!({"country":o.country,"zone":o.zone,"service":o.service,"per_item_price":o.per_item_price})).collect::<Vec<_>>() })).is_ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nullable_shipping_fields_are_explicit_and_missing_fields_fail_closed() {
        let fixtures: serde_json::Value =
            serde_json::from_str(include_str!("../fixtures/gallery-shipping.json")).unwrap();
        let value = fixtures["paintingBefore"].clone();
        let mut missing = value.clone();
        missing.as_object_mut().unwrap().remove("collection");
        assert!(serde_json::from_value::<ShippingDocument>(missing).is_err());
        let mut missing = value.clone();
        missing["countryOffers"][0]
            .as_object_mut()
            .unwrap()
            .remove("perItemPrice");
        assert!(serde_json::from_value::<ShippingDocument>(missing).is_err());
        let mut nullable = value;
        nullable["collection"] = serde_json::Value::Null;
        nullable["countryOffers"][0]["perItemPrice"] = serde_json::Value::Null;
        let document: ShippingDocument = serde_json::from_value(nullable.clone()).unwrap();
        assert!(document.validate());
        assert_eq!(nullable, serde_json::to_value(document).unwrap());
    }

    #[test]
    fn literal_safe_summary_matches_strict_camel_case_nested_mapping() {
        let pg: serde_json::Value = serde_json::from_str(include_str!(
            "../../shipping-json/fixtures/gallery-projection.json"
        ))
        .unwrap();
        let os: serde_json::Value =
            serde_json::from_str(include_str!("../fixtures/gallery-shipping.json")).unwrap();
        for key in ["paintingBefore", "paintingAfter", "cabinet"] {
            let summary = shipping_json::discovery::decode(pg[key].clone()).unwrap();
            let doc = ShippingDocument::from(&summary);
            assert!(doc.validate());
            assert_eq!(os[key], serde_json::to_value(doc).unwrap());
        }
        let mapping: serde_json::Value = serde_json::from_str(include_str!(
            "../../../opensearch/mappings/product_listings.json"
        ))
        .unwrap();
        let fields = &mapping["mappings"]["properties"]["shipping"];
        assert_eq!("strict", fields["dynamic"]);
        assert_eq!("nested", fields["properties"]["countryOffers"]["type"]);
        assert_eq!("strict", fields["properties"]["countryOffers"]["dynamic"]);
        assert_eq!(
            "unsigned_long",
            fields["properties"]["countryOffers"]["properties"]["perItemPrice"]["properties"]["amount"]
                ["type"]
        );
        let query = crate::shipping_query::country_offer_query(
            geo::CountryCode::DEU,
            Some((money::Currency::Eur, 8000)),
        );
        assert_eq!("shipping.countryOffers", query["nested"]["path"]);
        assert_eq!(
            3,
            query["nested"]["query"]["bool"]["filter"]
                .as_array()
                .unwrap()
                .len()
        );
    }
}
