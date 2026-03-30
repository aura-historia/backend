use crate::data::product_state_data::ProductStateData;
use common::price::data::PriceData;
use common::shops_product_id::ShopsProductId;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PatchProductData {
    pub shops_product_id: ShopsProductId,

    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub price: Option<PriceData>,

    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub state: Option<ProductStateData>,
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
    }

    #[test]
    fn should_roundtrip_serialize_deserialize_patch_product_data() {
        let data = PatchProductData {
            shops_product_id: ShopsProductId::from("round-trip".to_string()),
            price: Some(PriceData::new(CurrencyData::Usd, 500)),
            state: Some(ProductStateData::Sold),
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
        };
        let json = serde_json::to_value(&data).unwrap();
        assert!(json.get("state").is_none());
    }
}
