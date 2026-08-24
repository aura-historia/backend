use application::error::box_error;
use large_language_model::{
    BatchGenerationOptions, GenerationOptions, LargeLanguageModel, LargeLanguageModelError,
    StructuredGenerationRequest,
};
use localization::Language;
use product_listing_service::ports::ProductSearchFilterMatchSource;
use search_filter_core::enhanced_match_reason::EnhancedMatchReason;
use serde::Deserialize;
use std::num::NonZeroUsize;

const MAX_PRODUCT_MATCH_IMAGES: usize = 5;
const PRODUCT_MATCH_SYSTEM_INSTRUCTION: &str = "You are a product matching assistant for an antiques marketplace. Decide whether the product actually matches the requested search description using the product title, description, and optional product images. Return only JSON with a boolean `matches` and, when `matches` is true, a compact user-facing `reason` in the search language. Do not include markdown or extra fields.";

pub(crate) struct ProductMatchEvaluationRequest<'a, Key> {
    pub(crate) key: Key,
    pub(crate) product: &'a ProductSearchFilterMatchSource,
    pub(crate) search_description: &'a str,
    pub(crate) search_language: Language,
}

pub(crate) struct ProductMatchEvaluationResult<Key> {
    pub(crate) key: Key,
    pub(crate) outcome: ProductMatchEvaluationOutcome,
}

pub(crate) enum ProductMatchEvaluationOutcome {
    Matched(EnhancedMatchReason),
    Rejected,
    RetryableFailure(LargeLanguageModelError),
    PermanentFailure(LargeLanguageModelError),
}

#[derive(Debug, Deserialize)]
struct ProductMatchDecision {
    matches: bool,
    #[serde(default)]
    reason: Option<String>,
}

pub(crate) async fn evaluate_product_matches<E, Key>(
    llm: &E,
    evaluations: Vec<ProductMatchEvaluationRequest<'_, Key>>,
    max_concurrent_requests: NonZeroUsize,
) -> Vec<ProductMatchEvaluationResult<Key>>
where
    E: LargeLanguageModel,
{
    let (keys, requests): (Vec<_>, Vec<_>) = evaluations
        .into_iter()
        .map(|evaluation| {
            (
                evaluation.key,
                product_match_request(
                    evaluation.product,
                    evaluation.search_description,
                    evaluation.search_language,
                ),
            )
        })
        .unzip();
    let results = llm
        .generate_batch::<ProductMatchDecision>(
            requests,
            BatchGenerationOptions::new(max_concurrent_requests),
        )
        .await;

    keys.into_iter()
        .zip(results)
        .map(|(key, result)| ProductMatchEvaluationResult {
            key,
            outcome: match result.and_then(product_match_reason) {
                Ok(Some(reason)) => ProductMatchEvaluationOutcome::Matched(reason),
                Ok(None) => ProductMatchEvaluationOutcome::Rejected,
                Err(error) if is_retryable_llm_error(&error) => {
                    ProductMatchEvaluationOutcome::RetryableFailure(error)
                }
                Err(error) => ProductMatchEvaluationOutcome::PermanentFailure(error),
            },
        })
        .collect()
}

fn product_match_request(
    product: &ProductSearchFilterMatchSource,
    search_description: &str,
    search_language: Language,
) -> StructuredGenerationRequest {
    let (title, description) = product_text(product, search_language);
    let prompt = format!(
        "User's search description: {search_description}\nProduct title: {title}\nProduct description: {description}\nSearch language: {}\nReturn the reason in the search language.",
        search_language.format_human_readable(),
    );
    StructuredGenerationRequest {
        operation: large_language_model::LlmOperation::ProductEnhancedSearchDescriptionMatching,
        system_instruction: PRODUCT_MATCH_SYSTEM_INSTRUCTION.to_owned(),
        prompt,
        image_urls: product
            .images
            .iter()
            .take(MAX_PRODUCT_MATCH_IMAGES)
            .map(|image| image.url.clone())
            .collect(),
        response_json_schema: product_match_response_schema(),
        options: GenerationOptions {
            temperature: 0.0,
            max_output_tokens: 256,
            request_timeout: std::time::Duration::from_secs(30),
        },
    }
}

fn product_match_reason(
    decision: ProductMatchDecision,
) -> Result<Option<EnhancedMatchReason>, LargeLanguageModelError> {
    if !decision.matches {
        return Ok(None);
    }
    decision
        .reason
        .filter(|reason| !reason.trim().is_empty())
        .map(EnhancedMatchReason::from)
        .map(Some)
        .ok_or_else(|| LargeLanguageModelError::InvalidResponse {
            source: box_error(std::io::Error::other("matched response has no reason")),
        })
}

fn product_match_response_schema() -> serde_json::Value {
    serde_json::json!({
        "type": "OBJECT",
        "properties": {
            "matches": {"type": "BOOLEAN"},
            "reason": {"type": "STRING"}
        },
        "required": ["matches"]
    })
}

fn product_text(
    product: &ProductSearchFilterMatchSource,
    search_language: Language,
) -> (&str, &str) {
    let title = product
        .titles
        .get(&search_language)
        .or_else(|| product.titles.get(&Language::En))
        .map(AsRef::as_ref)
        .or_else(|| {
            product
                .product_title
                .as_ref()
                .map(|title| title.payload.as_ref())
        })
        .unwrap_or("");
    let description = product
        .descriptions
        .get(&search_language)
        .or_else(|| product.descriptions.get(&Language::En))
        .map(AsRef::as_ref)
        .unwrap_or("");
    (title, description)
}

fn is_retryable_llm_error(error: &LargeLanguageModelError) -> bool {
    matches!(
        error,
        LargeLanguageModelError::Timeout { .. }
            | LargeLanguageModelError::Retryable { .. }
            | LargeLanguageModelError::InvalidResponse { .. }
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use domain_primitives::event_id::EventId;
    use indexmap::IndexSet;
    use product_listing_core::{
        product::{ProductAddress, ProductAuction, ProductPricing},
        product_image::ProductImage,
        product_lifecycle::ProductLifecycle,
        product_slug_id::ProductSlugId,
        product_state::ProductState,
        shops_product_id::ShopsProductId,
    };
    use product_listing_service::ports::{
        ProductSearchFilterMatchShopType, ProductSearchFilterMatchSourceEventKind,
    };
    use shop_core::{shop_id::ShopId, shop_name::ShopName, shop_slug_id::ShopSlugId};
    use url::Url;

    fn product() -> Result<ProductSearchFilterMatchSource, url::ParseError> {
        let url = Url::parse("https://example.test/product")?;
        let event_id = EventId::new();
        Ok(ProductSearchFilterMatchSource {
            event_id,
            event_kind: ProductSearchFilterMatchSourceEventKind::Domain,
            origin_event_time: time::OffsetDateTime::UNIX_EPOCH,
            current_event_id: event_id,
            projection_version: 1,
            product_id: product_listing_core::product_id::ProductId::new(),
            product_slug_id: ProductSlugId::from("product"),
            shop_id: ShopId::new(),
            shop_slug_id: ShopSlugId::from("shop"),
            shop_name: ShopName::from("Shop"),
            shop_type: ProductSearchFilterMatchShopType::Marketplace,
            seller_id: ShopId::new(),
            seller_slug_id: shop_core::seller_slug_id::SellerSlugId::from("seller"),
            seller_name: ShopName::from("Seller"),
            shops_product_id: ShopsProductId::from("product"),
            address: ProductAddress::default(),
            product_title: None,
            product_description: None,
            titles: std::collections::HashMap::new(),
            descriptions: std::collections::HashMap::new(),
            pricing: ProductPricing::default(),
            sale_valuation: None,
            state: ProductState::Available,
            lifecycle: ProductLifecycle::Active,
            url: url.clone(),
            view_url: url,
            image: None,
            images: IndexSet::new(),
            embedding: None,
            auction: ProductAuction::default(),
            created: time::OffsetDateTime::UNIX_EPOCH,
            updated: time::OffsetDateTime::UNIX_EPOCH,
        })
    }

    #[test]
    fn should_include_description_localized_product_text_and_language_in_prompt()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut product = product()?;
        product.titles.insert(Language::En, "Brass lamp".into());
        product
            .descriptions
            .insert(Language::En, "From 1920".into());

        let request = product_match_request(&product, "vintage lighting", Language::En);

        assert!(request.prompt.contains("vintage lighting"));
        assert!(request.prompt.contains("Brass lamp"));
        assert!(request.prompt.contains("From 1920"));
        assert!(request.prompt.contains("English"));
        Ok(())
    }

    #[test]
    fn should_include_only_the_first_five_product_images_in_an_enhanced_request()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut product = product()?;
        let image_urls = (0..7)
            .map(|index| Url::parse(&format!("https://example.test/image-{index}.jpg")))
            .collect::<Result<Vec<_>, _>>()?;
        for url in &image_urls {
            product.images.insert(ProductImage {
                url: url.clone(),
                prohibited_content:
                    product_listing_core::prohibited_content::ProhibitedContent::None,
            });
        }

        let request = product_match_request(&product, "only paintings", Language::En);

        assert_eq!(
            &image_urls[..MAX_PRODUCT_MATCH_IMAGES],
            request.image_urls.as_slice()
        );
        Ok(())
    }

    #[test]
    fn should_reject_non_matching_response() -> Result<(), LargeLanguageModelError> {
        assert!(
            product_match_reason(ProductMatchDecision {
                matches: false,
                reason: Some("not relevant".to_owned()),
            })?
            .is_none()
        );
        Ok(())
    }

    #[test]
    fn should_reject_matched_response_without_reason() {
        let error = product_match_reason(ProductMatchDecision {
            matches: true,
            reason: None,
        });

        assert!(matches!(
            error,
            Err(LargeLanguageModelError::InvalidResponse { .. })
        ));
    }

    struct OrderedEvaluator;

    #[async_trait::async_trait]
    impl LargeLanguageModel for OrderedEvaluator {
        async fn generate<Output>(
            &self,
            request: StructuredGenerationRequest,
        ) -> Result<Output, LargeLanguageModelError>
        where
            Output: serde::de::DeserializeOwned + Send,
        {
            let response = if request.prompt.contains("matching request") {
                r#"{"matches":true,"reason":"matches"}"#
            } else {
                r#"{"matches":false}"#
            };
            serde_json::from_str(response).map_err(|source| {
                LargeLanguageModelError::InvalidResponse {
                    source: box_error(source),
                }
            })
        }
    }

    #[tokio::test]
    async fn should_preserve_request_key_order_when_mapping_batch_results()
    -> Result<(), Box<dyn std::error::Error>> {
        let product = product()?;
        let results = evaluate_product_matches(
            &OrderedEvaluator,
            vec![
                ProductMatchEvaluationRequest {
                    key: "first",
                    product: &product,
                    search_description: "matching request",
                    search_language: Language::En,
                },
                ProductMatchEvaluationRequest {
                    key: "second",
                    product: &product,
                    search_description: "rejected request",
                    search_language: Language::En,
                },
            ],
            NonZeroUsize::MIN,
        )
        .await;

        assert_eq!(results.len(), 2);
        assert_eq!(results[0].key, "first");
        assert!(matches!(
            results[0].outcome,
            ProductMatchEvaluationOutcome::Matched(_)
        ));
        assert_eq!(results[1].key, "second");
        assert!(matches!(
            results[1].outcome,
            ProductMatchEvaluationOutcome::Rejected
        ));
        Ok(())
    }

    #[test]
    fn should_classify_invalid_responses_as_retryable_and_permanent_errors_as_permanent() {
        let invalid_response = LargeLanguageModelError::InvalidResponse {
            source: box_error(std::io::Error::other("invalid response")),
        };
        let permanent = LargeLanguageModelError::Permanent {
            source: box_error(std::io::Error::other("invalid provider request")),
        };

        assert!(is_retryable_llm_error(&invalid_response));
        assert!(!is_retryable_llm_error(&permanent));
    }
}
