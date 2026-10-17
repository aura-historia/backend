//! Public discovery facts only. No dependency identity, location or buyer assessment.
use product_listing_service::shipping_projection::EffectiveListingShipping;
use serde::{Deserialize, Serialize};
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct ShippingDocument {
    pub collection: Option<String>,
    pub prohibited: bool,
    pub conditional: bool,
    pub country_offers: Vec<CountryOfferDocument>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct CountryOfferDocument {
    pub country: String,
    pub service: String,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub per_item_fixed: Option<AmountDocument>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct AmountDocument {
    pub currency: String,
    pub amount_minor: u64,
}
impl From<&EffectiveListingShipping> for ShippingDocument {
    fn from(s: &EffectiveListingShipping) -> Self {
        Self {
            collection: s.collection.map(|v| v.as_str().into()),
            prohibited: s.prohibited,
            conditional: s.conditional,
            country_offers: s
                .country_offers
                .iter()
                .map(|o| CountryOfferDocument {
                    country: o.country.alpha2().into(),
                    service: o.service.as_str().into(),
                    per_item_fixed: o.per_item_fixed.map(|p| AmountDocument {
                        currency: p.currency.as_str().into(),
                        amount_minor: p.monetary_amount.into(),
                    }),
                })
                .collect(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use product_listing_service::shipping_projection::derive;
    use shipping_core::{assessment::EffectiveDeliveryTerms, fixtures::gallery};
    #[test]
    fn literal_gallery_documents_and_strict_nested_mapping_preserve_minor_units() {
        let mut f = gallery();
        for (amount, expected) in [
            (
                8000,
                include_str!(
                    "../../product-listing-postgres/tests/fixtures/shipping/painting-opensearch-8000.json"
                ),
            ),
            (
                9500,
                include_str!(
                    "../../product-listing-postgres/tests/fixtures/shipping/painting-opensearch-9500.json"
                ),
            ),
        ] {
            let mut content = f.profile.content().clone();
            content.terms = shipping_core::fixtures::gallery_terms(amount);
            f.profile.replace_content(content).unwrap();
            let effective = EffectiveDeliveryTerms::resolve(
                &f.painting,
                Some(&f.source_default),
                std::slice::from_ref(&f.profile),
            );
            let document = ShippingDocument::from(&derive(&effective, &[]));
            assert_eq!(
                serde_json::from_str::<serde_json::Value>(expected).unwrap(),
                serde_json::to_value(document).unwrap()
            );
        }
        let cabinet = ShippingDocument::from(&derive(
            &EffectiveDeliveryTerms::resolve(
                &f.cabinet,
                Some(&f.source_default),
                std::slice::from_ref(&f.profile),
            ),
            &[],
        ));
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(include_str!(
                "../../product-listing-postgres/tests/fixtures/shipping/cabinet-opensearch.json"
            ))
            .unwrap(),
            serde_json::to_value(cabinet).unwrap()
        );
        let mapping: serde_json::Value = serde_json::from_str(include_str!(
            "../../../opensearch/mappings/product_listings.json"
        ))
        .unwrap();
        let shipping = &mapping["mappings"]["properties"]["shipping"];
        assert_eq!("strict", shipping["dynamic"]);
        assert_eq!("nested", shipping["properties"]["countryOffers"]["type"]);
        assert_eq!("strict", shipping["properties"]["countryOffers"]["dynamic"]);
        let mut poison: serde_json::Value = serde_json::from_str(include_str!(
            "../../product-listing-postgres/tests/fixtures/shipping/painting-opensearch-8000.json"
        ))
        .unwrap();
        poison["countryOffers"][0]["address"] = "private".into();
        assert!(serde_json::from_value::<ShippingDocument>(poison).is_err());
    }
}
