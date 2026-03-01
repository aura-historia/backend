pub use super::error::NormalizationError;
use super::{
    datetime::normalize_datetime_field,
    image::normalize_images,
    price::normalize_price_field,
    state::normalize_state,
    text::{normalize_description, normalize_shops_product_id, normalize_title_localized},
};
use crate::{
    css_selector::product_schema::RawExtractedProduct, normalization::product::NormalizedProduct,
};
use url::Url;

// ---------------------------------------------------------------------------
// Trait
// ---------------------------------------------------------------------------

#[async_trait::async_trait]
#[mockall::automock]
pub trait ProductNormalizationService {
    fn normalize(
        &self,
        raw: RawExtractedProduct,
        url: Url,
    ) -> Result<NormalizedProduct, NormalizationError>;
}

// ---------------------------------------------------------------------------
// Implementation
// ---------------------------------------------------------------------------

pub struct ProductNormalizationServiceImpl;

impl ProductNormalizationServiceImpl {
    pub fn new() -> Self {
        Self
    }
}

impl Default for ProductNormalizationServiceImpl {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait::async_trait]
impl ProductNormalizationService for ProductNormalizationServiceImpl {
    fn normalize(
        &self,
        raw: RawExtractedProduct,
        url: Url,
    ) -> Result<NormalizedProduct, NormalizationError> {
        let shops_product_id = normalize_shops_product_id(&raw.shops_product_id)?;
        let title = normalize_title_localized(&raw.title)?;
        let description = normalize_description(raw.description)?;

        let price = normalize_price_field(
            raw.price,
            |r| NormalizationError::PriceUnknownCurrency { raw: r },
            |r| NormalizationError::PriceParseError { raw: r },
        )?;
        let price_estimate_min = normalize_price_field(
            raw.price_estimate_min,
            |r| NormalizationError::PriceEstimateMinUnknownCurrency { raw: r },
            |r| NormalizationError::PriceEstimateMinParseError { raw: r },
        )?;
        let price_estimate_max = normalize_price_field(
            raw.price_estimate_max,
            |r| NormalizationError::PriceEstimateMaxUnknownCurrency { raw: r },
            |r| NormalizationError::PriceEstimateMaxParseError { raw: r },
        )?;

        let state = normalize_state(&raw.state);
        let images = normalize_images(raw.images, &url)?;

        let auction_start = normalize_datetime_field(raw.auction_start, |r| {
            NormalizationError::AuctionStartParseError { raw: r }
        })?;
        let auction_end = normalize_datetime_field(raw.auction_end, |r| {
            NormalizationError::AuctionEndParseError { raw: r }
        })?;

        Ok(NormalizedProduct {
            shops_product_id,
            title,
            description,
            price,
            price_estimate_min,
            price_estimate_max,
            state,
            url,
            images,
            auction_start,
            auction_end,
        })
    }
}

// ---------------------------------------------------------------------------
// Integration tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use time::macros::datetime;
    use url::Url;

    use common::{
        currency::domain::Currency,
        price::domain::{MonetaryAmount, Price},
        product_state::domain::ProductState,
    };

    use super::{NormalizationError, ProductNormalizationService, ProductNormalizationServiceImpl};
    use crate::css_selector::product_schema::RawExtractedProduct;

    fn base_url() -> Url {
        Url::parse("https://example.com/products/123").unwrap()
    }

    fn minimal_raw() -> RawExtractedProduct {
        RawExtractedProduct {
            shops_product_id: "PROD-001".into(),
            // Long enough for whatlang to reliably identify as English.
            title: "Antique ceramic vase from the early twentieth century in excellent condition"
                .into(),
            description: vec![],
            price: None,
            price_estimate_min: None,
            price_estimate_max: None,
            state: "available".into(),
            images: vec![],
            auction_start: None,
            auction_end: None,
        }
    }

    #[test]
    fn should_normalize_product_when_minimal_raw_provided() {
        let svc = ProductNormalizationServiceImpl::new();
        let result = svc.normalize(minimal_raw(), base_url()).unwrap();

        assert_eq!(result.shops_product_id.to_string(), "PROD-001");
        assert_eq!(
            result.title.payload.as_ref(),
            "Antique ceramic vase from the early twentieth century in excellent condition"
        );
        assert!(result.description.is_none());
        assert!(result.price.is_none());
        assert!(result.price_estimate_min.is_none());
        assert!(result.price_estimate_max.is_none());
        assert_eq!(result.state, ProductState::Available);
        assert!(result.images.is_empty());
        assert!(result.auction_start.is_none());
        assert!(result.auction_end.is_none());
    }

    #[test]
    fn should_normalize_product_when_full_raw_provided() {
        let svc = ProductNormalizationServiceImpl::new();
        let raw = RawExtractedProduct {
            shops_product_id: "LOT-42".into(),
            // Long enough English text for reliable language detection.
            title:
                "Victorian silver brooch in excellent original condition from private collection"
                    .into(),
            description: vec![
                "A beautiful antique brooch from the Victorian era.".into(),
                "In excellent original condition with no damage.".into(),
            ],
            price: Some("€ 1.200,00".into()),
            price_estimate_min: Some("£ 800.00".into()),
            price_estimate_max: Some("£1,200.00".into()),
            state: "listed".into(),
            images: vec![
                "https://cdn.example.com/img1.jpg".into(),
                "/img2.jpg".into(),
            ],
            auction_start: Some("2024-06-01T10:00:00Z".into()),
            auction_end: Some("2024-07-01T10:00:00Z".into()),
        };

        let result = svc.normalize(raw, base_url()).unwrap();

        assert_eq!(result.shops_product_id.to_string(), "LOT-42");
        assert_eq!(
            result.title.payload.as_ref(),
            "Victorian silver brooch in excellent original condition from private collection"
        );
        assert_eq!(
            result.description.unwrap().payload.as_ref(),
            "A beautiful antique brooch from the Victorian era.\n\nIn excellent original condition with no damage."
        );
        assert_eq!(
            result.price.unwrap(),
            Price::new(MonetaryAmount::from(120000u64), Currency::Eur)
        );
        assert_eq!(
            result.price_estimate_min.unwrap(),
            Price::new(MonetaryAmount::from(80000u64), Currency::Gbp)
        );
        assert_eq!(
            result.price_estimate_max.unwrap(),
            Price::new(MonetaryAmount::from(120000u64), Currency::Gbp)
        );
        assert_eq!(result.state, ProductState::Listed);
        assert_eq!(result.images.len(), 2);
        assert_eq!(
            result.auction_start.unwrap(),
            datetime!(2024-06-01 10:00:00 UTC)
        );
        assert_eq!(
            result.auction_end.unwrap(),
            datetime!(2024-07-01 10:00:00 UTC)
        );
    }

    #[test]
    fn should_return_error_when_shops_product_id_is_empty_for_normalize() {
        let svc = ProductNormalizationServiceImpl::new();
        let mut raw = minimal_raw();
        raw.shops_product_id = "  ".into();
        let err = svc.normalize(raw, base_url()).unwrap_err();
        assert!(matches!(err, NormalizationError::ShopsProductIdEmpty));
    }

    #[test]
    fn should_return_error_when_title_is_empty_for_normalize() {
        let svc = ProductNormalizationServiceImpl::new();
        let mut raw = minimal_raw();
        raw.title = "".into();
        let err = svc.normalize(raw, base_url()).unwrap_err();
        assert!(matches!(err, NormalizationError::TitleEmpty));
    }

    #[test]
    fn should_return_error_when_price_has_no_currency_for_normalize() {
        let svc = ProductNormalizationServiceImpl::new();
        let mut raw = minimal_raw();
        raw.price = Some("1234.56".into());
        let err = svc.normalize(raw, base_url()).unwrap_err();
        assert!(matches!(
            err,
            NormalizationError::PriceUnknownCurrency { .. }
        ));
    }

    #[test]
    fn should_return_error_when_price_is_unparseable_for_normalize() {
        let svc = ProductNormalizationServiceImpl::new();
        let mut raw = minimal_raw();
        raw.price = Some("€".into());
        let err = svc.normalize(raw, base_url()).unwrap_err();
        assert!(matches!(err, NormalizationError::PriceParseError { .. }));
    }

    #[test]
    fn should_return_error_when_price_estimate_min_has_no_currency_for_normalize() {
        let svc = ProductNormalizationServiceImpl::new();
        let mut raw = minimal_raw();
        raw.price_estimate_min = Some("800.00".into());
        let err = svc.normalize(raw, base_url()).unwrap_err();
        assert!(matches!(
            err,
            NormalizationError::PriceEstimateMinUnknownCurrency { .. }
        ));
    }

    #[test]
    fn should_return_error_when_price_estimate_min_is_unparseable_for_normalize() {
        let svc = ProductNormalizationServiceImpl::new();
        let mut raw = minimal_raw();
        raw.price_estimate_min = Some("£".into());
        let err = svc.normalize(raw, base_url()).unwrap_err();
        assert!(matches!(
            err,
            NormalizationError::PriceEstimateMinParseError { .. }
        ));
    }

    #[test]
    fn should_return_error_when_price_estimate_max_has_no_currency_for_normalize() {
        let svc = ProductNormalizationServiceImpl::new();
        let mut raw = minimal_raw();
        raw.price_estimate_max = Some("1200".into());
        let err = svc.normalize(raw, base_url()).unwrap_err();
        assert!(matches!(
            err,
            NormalizationError::PriceEstimateMaxUnknownCurrency { .. }
        ));
    }

    #[test]
    fn should_return_error_when_price_estimate_max_is_unparseable_for_normalize() {
        let svc = ProductNormalizationServiceImpl::new();
        let mut raw = minimal_raw();
        raw.price_estimate_max = Some("£".into());
        let err = svc.normalize(raw, base_url()).unwrap_err();
        assert!(matches!(
            err,
            NormalizationError::PriceEstimateMaxParseError { .. }
        ));
    }

    #[test]
    fn should_return_error_when_auction_start_is_unparseable_for_normalize() {
        let svc = ProductNormalizationServiceImpl::new();
        let mut raw = minimal_raw();
        raw.auction_start = Some("yesterday at noon".into());
        let err = svc.normalize(raw, base_url()).unwrap_err();
        assert!(matches!(
            err,
            NormalizationError::AuctionStartParseError { .. }
        ));
    }

    #[test]
    fn should_return_error_when_auction_end_is_unparseable_for_normalize() {
        let svc = ProductNormalizationServiceImpl::new();
        let mut raw = minimal_raw();
        raw.auction_end = Some("next tuesday".into());
        let err = svc.normalize(raw, base_url()).unwrap_err();
        assert!(matches!(
            err,
            NormalizationError::AuctionEndParseError { .. }
        ));
    }

    #[test]
    fn should_return_error_when_image_url_is_invalid_for_normalize() {
        let svc = ProductNormalizationServiceImpl::new();
        let mut raw = minimal_raw();
        raw.images = vec!["//".into()];
        let err = svc.normalize(raw, base_url()).unwrap_err();
        assert!(matches!(err, NormalizationError::InvalidImageUrl { .. }));
    }

    #[test]
    fn should_use_url_from_argument_as_product_url_when_normalizing() {
        let svc = ProductNormalizationServiceImpl::new();
        let url = Url::parse("https://shop.example.com/item/99").unwrap();
        let result = svc.normalize(minimal_raw(), url.clone()).unwrap();
        assert_eq!(result.url, url);
    }

    #[test]
    fn should_fallback_to_unknown_state_when_raw_state_not_in_lookup_table() {
        let svc = ProductNormalizationServiceImpl::new();
        let mut raw = minimal_raw();
        raw.state = "some_totally_unknown_state_xyz".into();
        let result = svc.normalize(raw, base_url()).unwrap();
        assert_eq!(result.state, ProductState::Unknown);
    }

    #[test]
    fn should_skip_none_price_fields_when_raw_prices_are_absent() {
        let svc = ProductNormalizationServiceImpl::new();
        let result = svc.normalize(minimal_raw(), base_url()).unwrap();
        assert!(result.price.is_none());
        assert!(result.price_estimate_min.is_none());
        assert!(result.price_estimate_max.is_none());
    }

    #[test]
    fn should_handle_empty_optional_price_string_when_raw_price_is_blank() {
        let svc = ProductNormalizationServiceImpl::new();
        let mut raw = minimal_raw();
        raw.price = Some("  ".into());
        // Blank string treated as absent — no currency error expected.
        let result = svc.normalize(raw, base_url()).unwrap();
        assert!(result.price.is_none());
    }

    #[test]
    fn should_handle_empty_optional_auction_string_when_raw_auction_is_blank() {
        let svc = ProductNormalizationServiceImpl::new();
        let mut raw = minimal_raw();
        raw.auction_start = Some("  ".into());
        raw.auction_end = Some("  ".into());
        let result = svc.normalize(raw, base_url()).unwrap();
        assert!(result.auction_start.is_none());
        assert!(result.auction_end.is_none());
    }
}
