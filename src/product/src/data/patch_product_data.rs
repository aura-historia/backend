use crate::data::authenticity_data::AuthenticityData;
use crate::data::condition_data::ConditionData;
use crate::data::origin_year_data::OriginYearData;
use crate::data::product_state_data::ProductStateData;
use crate::data::provenance_data::ProvenanceData;
use crate::data::restoration_data::RestorationData;
use common::price::data::PriceData;
use common::shops_product_id::ShopsProductId;
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use url::Url;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PatchProductData {
    pub shops_product_id: ShopsProductId,

    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub price: Option<PriceData>,

    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub state: Option<ProductStateData>,

    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub price_estimate_min: Option<PriceData>,

    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub price_estimate_max: Option<PriceData>,

    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub url: Option<Url>,

    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub images: Option<Vec<Url>>,

    #[serde(
        with = "time::serde::rfc3339::option",
        skip_serializing_if = "Option::is_none",
        default
    )]
    pub auction_start: Option<OffsetDateTime>,

    #[serde(
        with = "time::serde::rfc3339::option",
        skip_serializing_if = "Option::is_none",
        default
    )]
    pub auction_end: Option<OffsetDateTime>,

    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub origin_year: Option<OriginYearData>,

    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub authenticity: Option<AuthenticityData>,

    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub condition: Option<ConditionData>,

    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub provenance: Option<ProvenanceData>,

    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub restoration: Option<RestorationData>,
}

#[cfg(feature = "test-data")]
mod faker {
    use super::*;
    use fake::{Dummy, Fake, Faker as FakerStruct, RngExt};

    impl Dummy<FakerStruct> for PatchProductData {
        fn dummy_with_rng<R: RngExt + ?Sized>(config: &FakerStruct, rng: &mut R) -> Self {
            PatchProductData {
                shops_product_id: config.fake_with_rng(rng),
                price: config.fake_with_rng(rng),
                state: config.fake_with_rng(rng),
                price_estimate_min: config.fake_with_rng(rng),
                price_estimate_max: config.fake_with_rng(rng),
                url: Some(Url::parse("https://www.example.com/product/updated").unwrap()),
                images: Some(vec![Url::parse("https://www.example.com/img.jpg").unwrap()]),
                auction_start: if config.fake_with_rng(rng) {
                    Some(OffsetDateTime::now_utc())
                } else {
                    None
                },
                auction_end: if config.fake_with_rng(rng) {
                    Some(OffsetDateTime::now_utc())
                } else {
                    None
                },
                origin_year: config.fake_with_rng(rng),
                authenticity: config.fake_with_rng(rng),
                condition: config.fake_with_rng(rng),
                provenance: config.fake_with_rng(rng),
                restoration: config.fake_with_rng(rng),
            }
        }
    }

    #[cfg(test)]
    mod tests {
        use crate::data::patch_product_data::PatchProductData;
        use fake::{Fake, Faker};

        #[test]
        fn should_fake_patch_product_data() {
            let _ = Faker.fake::<PatchProductData>();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data::product_state_data::ProductStateData;
    use common::currency::data::CurrencyData;
    use rstest::rstest;

    #[test]
    fn should_serialize_patch_product_data_when_all_fields_present() {
        let data = PatchProductData {
            shops_product_id: ShopsProductId::from("abc-123".to_string()),
            price: Some(PriceData::new(CurrencyData::Eur, 1000)),
            state: Some(ProductStateData::Available),
            price_estimate_min: None,
            price_estimate_max: None,
            url: None,
            images: None,
            auction_start: None,
            auction_end: None,
            origin_year: None,
            authenticity: None,
            condition: None,
            provenance: None,
            restoration: None,
        };

        let json = serde_json::to_value(&data).unwrap();

        assert_eq!(json["shopsProductId"], "abc-123");
        assert_eq!(json["state"], "AVAILABLE");
        assert_eq!(json["price"]["currency"], "EUR");
        assert_eq!(json["price"]["amount"], 1000);
    }

    #[test]
    fn should_serialize_patch_product_data_when_price_is_none() {
        let data = PatchProductData {
            shops_product_id: ShopsProductId::from("abc-123".to_string()),
            price: None,
            state: Some(ProductStateData::Sold),
            price_estimate_min: None,
            price_estimate_max: None,
            url: None,
            images: None,
            auction_start: None,
            auction_end: None,
            origin_year: None,
            authenticity: None,
            condition: None,
            provenance: None,
            restoration: None,
        };

        let json = serde_json::to_value(&data).unwrap();

        assert_eq!(json["shopsProductId"], "abc-123");
        assert_eq!(json["state"], "SOLD");
        assert!(json.get("price").is_none());
    }

    #[test]
    fn should_deserialize_patch_product_data_when_minimal_fields() {
        let json = serde_json::json!({
            "shopsProductId": "abc-123"
        });

        let data: PatchProductData = serde_json::from_value(json).unwrap();

        assert_eq!(
            data.shops_product_id,
            ShopsProductId::from("abc-123".to_string())
        );
        assert!(data.state.is_none());
        assert!(data.price.is_none());
        assert!(data.price_estimate_min.is_none());
        assert!(data.price_estimate_max.is_none());
        assert!(data.url.is_none());
        assert!(data.images.is_none());
        assert!(data.auction_start.is_none());
        assert!(data.auction_end.is_none());
        assert!(data.origin_year.is_none());
        assert!(data.authenticity.is_none());
        assert!(data.condition.is_none());
        assert!(data.provenance.is_none());
        assert!(data.restoration.is_none());
    }

    #[test]
    fn should_roundtrip_serialize_deserialize_patch_product_data() {
        let data = PatchProductData {
            shops_product_id: ShopsProductId::from("round-trip".to_string()),
            price: Some(PriceData::new(CurrencyData::Usd, 500)),
            state: Some(ProductStateData::Sold),
            price_estimate_min: None,
            price_estimate_max: None,
            url: None,
            images: None,
            auction_start: None,
            auction_end: None,
            origin_year: None,
            authenticity: None,
            condition: None,
            provenance: None,
            restoration: None,
        };

        let json = serde_json::to_string(&data).unwrap();
        let deserialized: PatchProductData = serde_json::from_str(&json).unwrap();

        assert_eq!(data, deserialized);
    }

    #[rstest]
    #[case(Some(ProductStateData::Listed), "LISTED")]
    #[case(Some(ProductStateData::Available), "AVAILABLE")]
    #[case(Some(ProductStateData::Sold), "SOLD")]
    fn should_serialize_state_field_correctly_for_patch_product_data(
        #[case] state: Option<ProductStateData>,
        #[case] expected: &str,
    ) {
        let data = PatchProductData {
            shops_product_id: ShopsProductId::from("test".to_string()),
            price: None,
            state,
            price_estimate_min: None,
            price_estimate_max: None,
            url: None,
            images: None,
            auction_start: None,
            auction_end: None,
            origin_year: None,
            authenticity: None,
            condition: None,
            provenance: None,
            restoration: None,
        };
        let json = serde_json::to_value(&data).unwrap();
        assert_eq!(json["state"], expected);
    }

    #[test]
    fn should_skip_state_when_none_for_serialization() {
        let data = PatchProductData {
            shops_product_id: ShopsProductId::from("test".to_string()),
            price: None,
            state: None,
            price_estimate_min: None,
            price_estimate_max: None,
            url: None,
            images: None,
            auction_start: None,
            auction_end: None,
            origin_year: None,
            authenticity: None,
            condition: None,
            provenance: None,
            restoration: None,
        };
        let json = serde_json::to_value(&data).unwrap();
        assert!(json.get("state").is_none());
    }
}
