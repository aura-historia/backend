use crate::data::authenticity_data::AuthenticityData;
use crate::data::condition_data::ConditionData;
use crate::data::origin_year_data::OriginYearData;
use crate::data::product_state_data::ProductStateData;
use crate::data::provenance_data::ProvenanceData;
use crate::data::restoration_data::RestorationData;
use common::language::data::LocalizedTextData;
use common::price::data::PriceData;
use common::shops_product_id::ShopsProductId;
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use url::Url;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PostProductData {
    pub shops_product_id: ShopsProductId,
    pub title: LocalizedTextData,
    pub description: LocalizedTextData,

    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub price: Option<PriceData>,

    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub price_estimate_min: Option<PriceData>,

    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub price_estimate_max: Option<PriceData>,

    pub state: ProductStateData,
    pub url: Url,
    pub images: Vec<Url>,

    #[serde(
        skip_serializing_if = "Option::is_none",
        default,
        with = "time::serde::rfc3339::option"
    )]
    pub auction_start: Option<OffsetDateTime>,

    #[serde(
        skip_serializing_if = "Option::is_none",
        default,
        with = "time::serde::rfc3339::option"
    )]
    pub auction_end: Option<OffsetDateTime>,

    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub origin_year: Option<OriginYearData>,

    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub seller_name: Option<String>,

    #[serde(default)]
    pub authenticity: AuthenticityData,

    #[serde(default)]
    pub condition: ConditionData,

    #[serde(default)]
    pub provenance: ProvenanceData,

    #[serde(default)]
    pub restoration: RestorationData,
}

#[cfg(feature = "test-data")]
mod faker {
    use super::*;
    use fake::{Dummy, Fake, Faker as FakerStruct, RngExt};

    impl Dummy<FakerStruct> for PostProductData {
        fn dummy_with_rng<R: RngExt + ?Sized>(config: &FakerStruct, rng: &mut R) -> Self {
            PostProductData {
                shops_product_id: config.fake_with_rng(rng),
                title: config.fake_with_rng(rng),
                description: config.fake_with_rng(rng),
                price: config.fake_with_rng(rng),
                price_estimate_min: config.fake_with_rng(rng),
                price_estimate_max: config.fake_with_rng(rng),
                state: config.fake_with_rng(rng),
                url: Url::parse("https://www.example.com/product/1").unwrap(),
                images: vec![
                    Url::parse("https://www.example.com/image/1.jpg").unwrap(),
                    Url::parse("https://www.example.com/image/2.jpg").unwrap(),
                ],
                auction_start: None,
                auction_end: None,
                origin_year: None,
                seller_name: None,
                authenticity: Default::default(),
                condition: Default::default(),
                provenance: Default::default(),
                restoration: Default::default(),
            }
        }
    }

    #[cfg(test)]
    mod tests {
        use crate::data::post_product_data::PostProductData;
        use fake::{Fake, Faker};

        #[test]
        fn should_fake_post_product_data() {
            let _ = Faker.fake::<PostProductData>();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data::product_state_data::ProductStateData;
    use common::currency::data::CurrencyData;
    use common::language::data::{LanguageData, LocalizedTextData};

    #[test]
    fn should_serialize_post_product_data_when_all_fields_present() {
        let data = PostProductData {
            shops_product_id: ShopsProductId::from("abc-123".to_string()),
            title: LocalizedTextData::new("My Product", LanguageData::En),
            description: LocalizedTextData::new("A nice product", LanguageData::En),
            price: Some(PriceData::new(CurrencyData::Eur, 1000)),
            price_estimate_min: None,
            price_estimate_max: None,
            state: ProductStateData::Available,
            url: Url::parse("https://example.com/product").unwrap(),
            images: vec![Url::parse("https://example.com/img.jpg").unwrap()],
            auction_start: None,
            auction_end: None,
            origin_year: None,
            seller_name: None,
            authenticity: AuthenticityData::Unknown,
            condition: ConditionData::Unknown,
            provenance: ProvenanceData::Unknown,
            restoration: RestorationData::Unknown,
        };

        let json = serde_json::to_value(&data).unwrap();

        assert_eq!(json["shopsProductId"], "abc-123");
        assert_eq!(json["title"]["text"], "My Product");
        assert_eq!(json["state"], "AVAILABLE");
        assert_eq!(json["price"]["currency"], "EUR");
        assert_eq!(json["price"]["amount"], 1000);
        assert!(json.get("priceEstimateMin").is_none());
        assert!(json.get("auctionStart").is_none());
    }

    #[test]
    fn should_deserialize_post_product_data_when_minimal_fields() {
        let json = serde_json::json!({
            "shopsProductId": "abc-123",
            "title": { "text": "Title", "language": "en" },
            "description": { "text": "Desc", "language": "en" },
            "state": "LISTED",
            "url": "https://example.com",
            "images": []
        });

        let data: PostProductData = serde_json::from_value(json).unwrap();

        assert_eq!(
            data.shops_product_id,
            ShopsProductId::from("abc-123".to_string())
        );
        assert_eq!(data.state, ProductStateData::Listed);
        assert_eq!(data.authenticity, AuthenticityData::Unknown);
        assert_eq!(data.condition, ConditionData::Unknown);
        assert_eq!(data.provenance, ProvenanceData::Unknown);
        assert_eq!(data.restoration, RestorationData::Unknown);
        assert!(data.price.is_none());
        assert!(data.auction_start.is_none());
    }

    #[test]
    fn should_roundtrip_serialize_deserialize_post_product_data() {
        let data = PostProductData {
            shops_product_id: ShopsProductId::from("round-trip".to_string()),
            title: LocalizedTextData::new("Round Trip Title", LanguageData::De),
            description: LocalizedTextData::new("Round Trip Desc", LanguageData::De),
            price: Some(PriceData::new(CurrencyData::Usd, 500)),
            price_estimate_min: Some(PriceData::new(CurrencyData::Usd, 400)),
            price_estimate_max: Some(PriceData::new(CurrencyData::Usd, 600)),
            state: ProductStateData::Sold,
            url: Url::parse("https://example.com/roundtrip").unwrap(),
            images: vec![],
            auction_start: None,
            auction_end: None,
            origin_year: Some(OriginYearData {
                min: Some(1800.into()),
                year: None,
                max: Some(1900.into()),
            }),
            seller_name: None,
            authenticity: AuthenticityData::Original,
            condition: ConditionData::Good,
            provenance: ProvenanceData::Complete,
            restoration: RestorationData::Minor,
        };

        let json = serde_json::to_string(&data).unwrap();
        let deserialized: PostProductData = serde_json::from_str(&json).unwrap();

        assert_eq!(data, deserialized);
    }
}
