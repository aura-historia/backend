use common::{
    enhanced_match_reason::EnhancedMatchReason,
    error::boxed::box_error,
    logging::{
        GeminiServiceTier, LlmInvocationMetrics, LlmModel, LlmOperation, LlmProvider,
        log_llm_invocation,
    },
};
use embedding::{FetchedImage, SafeImageFetcher};
use futures::{StreamExt, stream};
use google_cloud_auth::credentials::AccessTokenCredentials;
use product_service::ports::ProductSearchFilterMatchSource;
use search_filter_service::ports::{
    ProductMatchEvaluation, ProductMatchEvaluator, ProductMatchEvaluatorError, SearchFilterView,
};
use serde::{Deserialize, Serialize};
use std::{sync::Arc, time::Duration};

const GEMINI_MODEL: &str = "gemini-3.1-flash-lite";
const MAX_PRODUCT_IMAGES: usize = 5;
const MAX_CONCURRENT_VERTEX_REQUESTS: usize = 4;
const VERTEX_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const VERTEX_REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
const SYSTEM_INSTRUCTION: &str = "You are a product matching assistant for an antiques marketplace. Decide whether the product actually matches the requested search description using the product title, description, and optional product images. Return only JSON with a boolean `matches` and, when `matches` is true, a compact user-facing `reason` in the search language. Do not include markdown or extra fields.";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VertexAiProductMatchEvaluatorConfig {
    project_id: String,
    location: String,
}

impl VertexAiProductMatchEvaluatorConfig {
    pub fn new(project_id: impl Into<String>, location: impl Into<String>) -> Self {
        Self {
            project_id: project_id.into(),
            location: location.into(),
        }
    }

    pub fn project_id(&self) -> &str {
        &self.project_id
    }

    pub fn location(&self) -> &str {
        &self.location
    }
}

pub struct VertexAiProductMatchEvaluator {
    generate_content_url: String,
    client: reqwest::Client,
    image_fetcher: SafeImageFetcher,
    credentials: AccessTokenCredentials,
}

impl VertexAiProductMatchEvaluator {
    pub fn new(
        config: VertexAiProductMatchEvaluatorConfig,
        credentials: AccessTokenCredentials,
    ) -> Self {
        Self {
            generate_content_url: build_generate_content_url(&config),
            client: reqwest::Client::builder()
                .connect_timeout(VERTEX_CONNECT_TIMEOUT)
                .timeout(VERTEX_REQUEST_TIMEOUT)
                .build()
                .unwrap_or_else(|_| reqwest::Client::new()),
            image_fetcher: SafeImageFetcher::new(),
            credentials,
        }
    }

    async fn request(
        &self,
        product: &ProductSearchFilterMatchSource,
        filter: &SearchFilterView,
        image_contents: &[ProviderContent],
        access_token: &str,
    ) -> Result<ProductMatchEvaluation, ProductMatchEvaluatorError> {
        let request = GenerateContentRequest::new(product, filter, image_contents)?;
        let started_at = std::time::Instant::now();
        let response = self
            .client
            .post(&self.generate_content_url)
            .bearer_auth(access_token)
            .json(&request)
            .send()
            .await
            .map_err(request_error)?;
        let status = response.status();
        if !status.is_success() {
            return Err(http_status_error(status));
        }
        let response = response
            .json::<GenerateContentResponse>()
            .await
            .map_err(invalid_response_error)?;
        log_llm_invocation(
            LlmOperation::ProductEnhancedSearchDescriptionMatching,
            LlmProvider::Google,
            LlmModel::Gemini31FlashLite,
            started_at.elapsed(),
            response.usage_metrics(),
        );
        response.into_evaluation()
    }

    async fn product_image_contents(
        &self,
        product: &ProductSearchFilterMatchSource,
    ) -> Vec<ProviderContent> {
        let mut contents = Vec::with_capacity(MAX_PRODUCT_IMAGES);
        for image in first_product_images(product) {
            if let Some(image) = self.image_fetcher.fetch(&image.url).await {
                contents.push(ProviderContent::image(image));
            }
        }
        contents
    }
}

#[async_trait::async_trait]
impl ProductMatchEvaluator for VertexAiProductMatchEvaluator {
    async fn evaluate(
        &self,
        product: &ProductSearchFilterMatchSource,
        filter: &SearchFilterView,
    ) -> Result<ProductMatchEvaluation, ProductMatchEvaluatorError> {
        let mut results = self
            .evaluate_batch(product, std::slice::from_ref(filter))
            .await;
        results
            .pop()
            .ok_or_else(|| invalid_response_error(std::io::Error::other("empty evaluator batch")))?
            .result
    }

    async fn evaluate_batch(
        &self,
        product: &ProductSearchFilterMatchSource,
        filters: &[SearchFilterView],
    ) -> Vec<search_filter_service::ports::ProductMatchEvaluationResult> {
        let image_contents = Arc::new(self.product_image_contents(product).await);
        let access_token = match self.credentials.access_token().await {
            Ok(credentials) => credentials.token,
            Err(source) => {
                let message = source.to_string();
                return filters
                    .iter()
                    .map(
                        |filter| search_filter_service::ports::ProductMatchEvaluationResult {
                            search_filter_id: filter.search_filter_id,
                            result: Err(retryable_error(std::io::Error::other(message.clone()))),
                        },
                    )
                    .collect();
            }
        };

        let access_token = Arc::new(access_token);
        stream::iter(filters.iter().cloned())
            .map(|filter| {
                let image_contents = Arc::clone(&image_contents);
                let access_token = Arc::clone(&access_token);
                async move {
                    let result = self
                        .request(
                            product,
                            &filter,
                            image_contents.as_ref(),
                            access_token.as_str(),
                        )
                        .await;
                    search_filter_service::ports::ProductMatchEvaluationResult {
                        search_filter_id: filter.search_filter_id,
                        result,
                    }
                }
            })
            .buffer_unordered(MAX_CONCURRENT_VERTEX_REQUESTS)
            .collect()
            .await
    }
}

fn build_generate_content_url(config: &VertexAiProductMatchEvaluatorConfig) -> String {
    let endpoint = match config.location() {
        "us" | "eu" => format!(
            "https://aiplatform.{}.rep.googleapis.com",
            config.location()
        ),
        "global" => "https://aiplatform.googleapis.com".to_owned(),
        location => format!("https://{location}-aiplatform.googleapis.com"),
    };
    format!(
        "{endpoint}/v1/projects/{}/locations/{}/publishers/google/models/{GEMINI_MODEL}:generateContent",
        config.project_id(),
        config.location(),
    )
}

fn retryable_error(
    source: impl std::error::Error + Send + Sync + 'static,
) -> ProductMatchEvaluatorError {
    ProductMatchEvaluatorError::Retryable {
        source: box_error(source),
    }
}

fn request_error(source: reqwest::Error) -> ProductMatchEvaluatorError {
    if source.is_timeout() {
        ProductMatchEvaluatorError::Timeout {
            source: box_error(source),
        }
    } else {
        retryable_error(source)
    }
}

fn invalid_response_error(
    source: impl std::error::Error + Send + Sync + 'static,
) -> ProductMatchEvaluatorError {
    ProductMatchEvaluatorError::InvalidResponse {
        source: box_error(source),
    }
}

fn http_status_error(status: reqwest::StatusCode) -> ProductMatchEvaluatorError {
    let source = std::io::Error::other(format!("Vertex AI returned HTTP {status}"));
    if status.as_u16() == 429 || status.is_server_error() || status.as_u16() == 408 {
        retryable_error(source)
    } else {
        ProductMatchEvaluatorError::Permanent {
            source: box_error(source),
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct GenerateContentRequest {
    system_instruction: ProviderContent,
    contents: Vec<ProviderContent>,
    generation_config: GenerationConfig,
}

impl GenerateContentRequest {
    fn new(
        product: &ProductSearchFilterMatchSource,
        filter: &SearchFilterView,
        image_contents: &[ProviderContent],
    ) -> Result<Self, ProductMatchEvaluatorError> {
        let description = filter
            .search
            .enhanced_search_description
            .as_ref()
            .ok_or_else(|| {
                invalid_response_error(std::io::Error::other("filter has no enhanced description"))
            })?;
        let language = filter.search.language;
        let (title, product_description) = product_text(product, language);
        let prompt = format!(
            "User's search description: {description}\nProduct title: {title}\nProduct description: {product_description}\nSearch language: {}\nReturn the reason in the search language.",
            language.format_human_readable(),
        );
        let mut contents = Vec::with_capacity(image_contents.len() + 1);
        contents.push(ProviderContent::text(prompt));
        contents.extend_from_slice(image_contents);
        Ok(Self {
            system_instruction: ProviderContent::text(SYSTEM_INSTRUCTION),
            contents,
            generation_config: GenerationConfig {
                temperature: 0.0,
                max_output_tokens: 256,
                response_mime_type: "application/json",
                response_schema: ProductMatchResponseSchema::default(),
            },
        })
    }
}

fn first_product_images(
    product: &ProductSearchFilterMatchSource,
) -> impl Iterator<Item = &product_core::product_image::ProductImage> {
    product.images.iter().take(MAX_PRODUCT_IMAGES)
}

fn product_text(
    product: &ProductSearchFilterMatchSource,
    search_language: common::language::domain::Language,
) -> (&str, &str) {
    let title = product
        .titles
        .get(&search_language)
        .or_else(|| product.titles.get(&common::language::domain::Language::En))
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
        .or_else(|| {
            product
                .descriptions
                .get(&common::language::domain::Language::En)
        })
        .map(AsRef::as_ref)
        .unwrap_or("");
    (title, description)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ProviderContent {
    parts: Vec<ProviderPart>,
}

impl ProviderContent {
    fn text(text: impl Into<String>) -> Self {
        Self {
            parts: vec![ProviderPart::Text { text: text.into() }],
        }
    }

    fn image(image: FetchedImage) -> Self {
        Self::inline_image(image.mime_type(), image.base64_data())
    }

    fn inline_image(mime_type: impl Into<String>, data: impl Into<String>) -> Self {
        Self {
            parts: vec![ProviderPart::InlineData {
                inline_data: ProviderInlineData {
                    mime_type: mime_type.into(),
                    data: data.into(),
                },
            }],
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
enum ProviderPart {
    Text {
        text: String,
    },
    InlineData {
        #[serde(rename = "inlineData")]
        inline_data: ProviderInlineData,
    },
}

impl ProviderPart {
    fn into_text(self) -> Option<String> {
        match self {
            Self::Text { text } => Some(text),
            Self::InlineData { .. } => None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ProviderInlineData {
    mime_type: String,
    data: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct GenerationConfig {
    temperature: f32,
    max_output_tokens: u16,
    response_mime_type: &'static str,
    response_schema: ProductMatchResponseSchema,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ProductMatchResponseSchema {
    schema_type: &'static str,
    properties: serde_json::Value,
    required: [&'static str; 1],
}

impl Default for ProductMatchResponseSchema {
    fn default() -> Self {
        Self {
            schema_type: "OBJECT",
            properties: serde_json::json!({
                "matches": {"type": "BOOLEAN"},
                "reason": {"type": "STRING"}
            }),
            required: ["matches"],
        }
    }
}

#[derive(Debug, Deserialize)]
struct GenerateContentResponse {
    #[serde(default)]
    candidates: Vec<ProviderCandidate>,
    #[serde(default)]
    usage_metadata: ProviderUsageMetadata,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ProviderUsageMetadata {
    prompt_token_count: Option<u32>,
    candidates_token_count: Option<u32>,
    total_token_count: Option<u32>,
    cached_content_token_count: Option<u32>,
    thoughts_token_count: Option<u32>,
}

impl GenerateContentResponse {
    fn usage_metrics(&self) -> LlmInvocationMetrics {
        LlmInvocationMetrics {
            service_tier: Some(GeminiServiceTier::Standard),
            prompt_tokens: self.usage_metadata.prompt_token_count,
            completion_tokens: self.usage_metadata.candidates_token_count,
            total_tokens: self.usage_metadata.total_token_count,
            cached_prompt_tokens: self.usage_metadata.cached_content_token_count,
            reasoning_tokens: self.usage_metadata.thoughts_token_count,
            ..Default::default()
        }
    }

    fn into_evaluation(self) -> Result<ProductMatchEvaluation, ProductMatchEvaluatorError> {
        let text = self
            .candidates
            .into_iter()
            .find_map(|candidate| candidate.content)
            .and_then(|content| content.parts.into_iter().find_map(ProviderPart::into_text))
            .ok_or_else(|| {
                invalid_response_error(std::io::Error::other("Vertex AI response has no content"))
            })?;
        let decision =
            serde_json::from_str::<ProductMatchDecision>(&text).map_err(invalid_response_error)?;
        if !decision.matches {
            return Ok(ProductMatchEvaluation::NotMatched);
        }
        let reason = decision
            .reason
            .filter(|reason| !reason.trim().is_empty())
            .ok_or_else(|| {
                invalid_response_error(std::io::Error::other("matched response has no reason"))
            })?;
        Ok(ProductMatchEvaluation::Matched {
            reason: Some(EnhancedMatchReason::from(reason)),
        })
    }
}

#[derive(Debug, Deserialize)]
struct ProviderCandidate {
    content: Option<ProviderContent>,
}

#[derive(Debug, Deserialize)]
struct ProductMatchDecision {
    matches: bool,
    #[serde(default)]
    reason: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use common::{
        currency::domain::Currency, event_id::EventId, language::domain::Language,
        product_lifecycle::domain::ProductLifecycle, product_slug_id::ProductSlugId,
        product_state::domain::ProductState, resource_state::domain::ResourceState,
        seller_slug_id::SellerSlugId, shop_id::ShopId, shop_name::ShopName,
        shop_slug_id::ShopSlugId, shops_product_id::ShopsProductId, user_id::UserId,
        user_search_filter_id::UserSearchFilterId, user_search_filter_name::UserSearchFilterName,
    };
    use indexmap::IndexSet;
    use product_core::{
        product::{ProductAddress, ProductAuction, ProductPricing},
        product_image::ProductImage,
        prohibited_content::ProhibitedContent,
        title::Title,
    };
    use search_filter_core::ProductSearch;
    use search_filter_service::ports::ProductMatchEvaluatorError;
    use std::collections::HashMap;
    use url::Url;

    #[test]
    fn should_build_regional_and_global_vertex_endpoints() {
        assert_eq!(
            build_generate_content_url(&VertexAiProductMatchEvaluatorConfig::new("project", "eu")),
            "https://aiplatform.eu.rep.googleapis.com/v1/projects/project/locations/eu/publishers/google/models/gemini-3.1-flash-lite:generateContent"
        );
        assert_eq!(
            build_generate_content_url(&VertexAiProductMatchEvaluatorConfig::new(
                "project", "global"
            )),
            "https://aiplatform.googleapis.com/v1/projects/project/locations/global/publishers/google/models/gemini-3.1-flash-lite:generateContent"
        );
    }

    #[test]
    fn should_include_only_the_first_five_product_images_in_vertex_request()
    -> Result<(), Box<dyn std::error::Error>> {
        let product = product_source()?;
        let filter = enhanced_filter();
        let image_contents = first_product_images(&product)
            .map(|image| ProviderContent::inline_image("image/png", image.url.as_str()))
            .collect::<Vec<_>>();

        let request = GenerateContentRequest::new(&product, &filter, &image_contents)?;
        let request = serde_json::to_value(request)?;
        let contents = request["contents"]
            .as_array()
            .ok_or_else(|| std::io::Error::other("Vertex request contents are missing"))?;

        assert_eq!(MAX_PRODUCT_IMAGES + 1, contents.len());
        assert!(
            contents[0]["parts"][0]["text"]
                .as_str()
                .is_some_and(|text| text.contains("English product title"))
        );
        for (index, image) in product.images.iter().take(MAX_PRODUCT_IMAGES).enumerate() {
            assert_eq!(
                image.url.as_str(),
                contents[index + 1]["parts"][0]["inlineData"]["data"]
            );
        }
        assert!(request.to_string().contains("product-image-4"));
        assert!(!request.to_string().contains("product-image-5"));
        assert!(!request.to_string().contains("product-image-6"));
        Ok(())
    }

    #[test]
    fn should_accept_structured_match_response() -> Result<(), ProductMatchEvaluatorError> {
        let response = GenerateContentResponse {
            candidates: vec![ProviderCandidate {
                content: Some(ProviderContent::text(
                    r#"{"matches":true,"reason":"The title and description match."}"#,
                )),
            }],
            usage_metadata: ProviderUsageMetadata::default(),
        };

        assert!(matches!(
            response.into_evaluation()?,
            ProductMatchEvaluation::Matched { reason: Some(_) }
        ));
        Ok(())
    }

    #[test]
    fn should_request_reason_in_filter_search_language() -> Result<(), Box<dyn std::error::Error>> {
        let product = product_source()?;
        let mut filter = enhanced_filter();
        filter.search.language = Language::De;

        let request = GenerateContentRequest::new(&product, &filter, &[])?;
        let request = serde_json::to_value(request)?;
        let prompt = request["contents"][0]["parts"][0]["text"]
            .as_str()
            .ok_or_else(|| std::io::Error::other("Vertex prompt is missing"))?;

        assert!(prompt.contains("Search language: German"));
        assert!(prompt.contains("Return the reason in the search language."));
        Ok(())
    }

    #[test]
    fn should_classify_transient_and_permanent_vertex_statuses() {
        assert!(matches!(
            http_status_error(reqwest::StatusCode::TOO_MANY_REQUESTS),
            ProductMatchEvaluatorError::Retryable { .. }
        ));
        assert!(matches!(
            http_status_error(reqwest::StatusCode::INTERNAL_SERVER_ERROR),
            ProductMatchEvaluatorError::Retryable { .. }
        ));
        assert!(matches!(
            http_status_error(reqwest::StatusCode::BAD_REQUEST),
            ProductMatchEvaluatorError::Permanent { .. }
        ));
    }

    #[test]
    fn should_reject_matched_response_without_reason() {
        let response = GenerateContentResponse {
            candidates: vec![ProviderCandidate {
                content: Some(ProviderContent::text(r#"{"matches":true}"#)),
            }],
            usage_metadata: ProviderUsageMetadata::default(),
        };

        assert!(response.into_evaluation().is_err());
    }

    fn product_source() -> Result<ProductSearchFilterMatchSource, url::ParseError> {
        let images = (0..7)
            .map(|index| {
                Ok(ProductImage {
                    url: Url::parse(&format!(
                        "https://images.example.test/product-image-{index}.png"
                    ))?,
                    prohibited_content: ProhibitedContent::None,
                })
            })
            .collect::<Result<IndexSet<_>, url::ParseError>>()?;
        let event_id = EventId::new();
        let url = Url::parse("https://shop.example.test/products/product")?;
        Ok(ProductSearchFilterMatchSource {
            event_id,
            event_kind: product_service::ports::ProductSearchFilterMatchSourceEventKind::Domain,
            current_event_id: event_id,
            product_id: common::product_id::ProductId::new(),
            product_slug_id: ProductSlugId::from("product"),
            shop_id: ShopId::new(),
            shop_slug_id: ShopSlugId::from("shop"),
            shop_name: ShopName::from("Shop"),
            shop_type: product_service::ports::ProductSearchFilterMatchShopType::Marketplace,
            seller_id: ShopId::new(),
            seller_slug_id: SellerSlugId::from("seller"),
            seller_name: ShopName::from("Seller"),
            shops_product_id: ShopsProductId::from("sku-1"),
            address: ProductAddress::default(),
            product_title: Some(common::localized::Localized::new(
                Language::De,
                Title::from("Native product title"),
            )),
            product_description: None,
            titles: HashMap::from([(Language::En, Title::from("English product title"))]),
            descriptions: HashMap::from([(
                Language::En,
                product_core::description::Description::from("English product description"),
            )]),
            pricing: ProductPricing::default(),
            state: ProductState::Available,
            lifecycle: ProductLifecycle::Active,
            url: url.clone(),
            view_url: url,
            image: images.first().cloned(),
            images,
            auction: ProductAuction::default(),
            created: time::OffsetDateTime::UNIX_EPOCH,
            updated: time::OffsetDateTime::UNIX_EPOCH,
        })
    }

    fn enhanced_filter() -> SearchFilterView {
        SearchFilterView {
            search_filter_id: UserSearchFilterId::new(),
            user_id: UserId::new(),
            name: UserSearchFilterName::from("enhanced"),
            notifications: true,
            state: ResourceState::Active,
            search: ProductSearch::new(Language::En, Currency::Eur)
                .with_enhanced_search_description("brass lamp".into()),
            embedding: None,
            created: time::OffsetDateTime::UNIX_EPOCH,
            updated: time::OffsetDateTime::UNIX_EPOCH,
            last_hybrid_search_matched: time::OffsetDateTime::UNIX_EPOCH,
        }
    }
}
