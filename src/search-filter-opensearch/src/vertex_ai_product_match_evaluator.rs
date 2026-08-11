use common::{
    enhanced_match_reason::EnhancedMatchReason,
    error::boxed::box_error,
    logging::{
        GeminiServiceTier, LlmInvocationMetrics, LlmModel, LlmOperation, LlmProvider,
        log_llm_invocation,
    },
};
use embedding::{FetchedImage, SafeImageFetcher};
use google_cloud_auth::credentials::AccessTokenCredentials;
use product_service::ports::ProductSearchFilterMatchSource;
use search_filter_service::ports::{
    ProductMatchEvaluation, ProductMatchEvaluator, ProductMatchEvaluatorError, SearchFilterView,
};
use serde::{Deserialize, Serialize};

const GEMINI_MODEL: &str = "gemini-3.1-flash-lite";
const MAX_PRODUCT_IMAGES: usize = 5;
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
            client: reqwest::Client::new(),
            image_fetcher: SafeImageFetcher::new(),
            credentials,
        }
    }

    async fn request(
        &self,
        product: &ProductSearchFilterMatchSource,
        filter: &SearchFilterView,
    ) -> Result<ProductMatchEvaluation, ProductMatchEvaluatorError> {
        let image_contents = self.product_image_contents(product).await;
        let request = GenerateContentRequest::new(product, filter, image_contents)?;
        let access_token = self
            .credentials
            .access_token()
            .await
            .map_err(evaluation_error)?;
        let started_at = std::time::Instant::now();
        let response = self
            .client
            .post(&self.generate_content_url)
            .bearer_auth(access_token.token)
            .json(&request)
            .send()
            .await
            .map_err(evaluation_error)?;
        let status = response.status();
        if !status.is_success() {
            return Err(evaluation_error(std::io::Error::other(format!(
                "Vertex AI returned HTTP {status}"
            ))));
        }
        let response = response
            .json::<GenerateContentResponse>()
            .await
            .map_err(evaluation_error)?;
        log_llm_invocation(
            LlmOperation::ProductEnhancedSearchDescriptionMatching,
            LlmProvider::Google,
            LlmModel::Gemini31FlashLite,
            started_at.elapsed(),
            LlmInvocationMetrics {
                service_tier: Some(GeminiServiceTier::Standard),
                ..Default::default()
            },
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
        self.request(product, filter).await
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

fn evaluation_error(
    source: impl std::error::Error + Send + Sync + 'static,
) -> ProductMatchEvaluatorError {
    ProductMatchEvaluatorError::EvaluationFailed {
        source: box_error(source),
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
        image_contents: Vec<ProviderContent>,
    ) -> Result<Self, ProductMatchEvaluatorError> {
        let description = filter
            .search
            .enhanced_search_description
            .as_ref()
            .ok_or_else(|| {
                evaluation_error(std::io::Error::other("filter has no enhanced description"))
            })?;
        let (title, product_description) = product_text(product);
        let prompt = format!(
            "User's search description: {description}\nProduct title: {title}\nProduct description: {product_description}\nUser's preferred language: {}",
            filter.search.language.format_human_readable(),
        );
        let mut contents = Vec::with_capacity(image_contents.len() + 1);
        contents.push(ProviderContent::text(prompt));
        contents.extend(image_contents);
        Ok(Self {
            system_instruction: ProviderContent::text(SYSTEM_INSTRUCTION),
            contents,
            generation_config: GenerationConfig {
                temperature: 0.0,
                max_output_tokens: 256,
                response_mime_type: "application/json",
            },
        })
    }
}

fn first_product_images(
    product: &ProductSearchFilterMatchSource,
) -> impl Iterator<Item = &product_core::product_image::ProductImage> {
    product.images.iter().take(MAX_PRODUCT_IMAGES)
}

fn product_text(product: &ProductSearchFilterMatchSource) -> (&str, &str) {
    let title = product
        .titles
        .get(&common::language::domain::Language::En)
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
        .get(&common::language::domain::Language::En)
        .map(AsRef::as_ref)
        .unwrap_or("");
    (title, description)
}

#[derive(Debug, Serialize, Deserialize)]
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

#[derive(Debug, Serialize, Deserialize)]
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

#[derive(Debug, Serialize, Deserialize)]
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
}

#[derive(Debug, Deserialize)]
struct GenerateContentResponse {
    #[serde(default)]
    candidates: Vec<ProviderCandidate>,
}

impl GenerateContentResponse {
    fn into_evaluation(self) -> Result<ProductMatchEvaluation, ProductMatchEvaluatorError> {
        let text = self
            .candidates
            .into_iter()
            .find_map(|candidate| candidate.content)
            .and_then(|content| content.parts.into_iter().find_map(ProviderPart::into_text))
            .ok_or_else(|| {
                evaluation_error(std::io::Error::other("Vertex AI response has no content"))
            })?;
        let decision =
            serde_json::from_str::<ProductMatchDecision>(&text).map_err(evaluation_error)?;
        if !decision.matches {
            return Ok(ProductMatchEvaluation::NotMatched);
        }
        let reason = decision
            .reason
            .filter(|reason| !reason.trim().is_empty())
            .ok_or_else(|| {
                evaluation_error(std::io::Error::other("matched response has no reason"))
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
            .collect();

        let request = GenerateContentRequest::new(&product, &filter, image_contents)?;
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
        };

        assert!(matches!(
            response.into_evaluation()?,
            ProductMatchEvaluation::Matched { reason: Some(_) }
        ));
        Ok(())
    }

    #[test]
    fn should_preserve_legacy_english_text_preference() -> Result<(), Box<dyn std::error::Error>> {
        let product = product_source()?;

        let (title, description) = product_text(&product);

        assert_eq!("English product title", title);
        assert_eq!("English product description", description);
        Ok(())
    }

    #[test]
    fn should_reject_matched_response_without_reason() {
        let response = GenerateContentResponse {
            candidates: vec![ProviderCandidate {
                content: Some(ProviderContent::text(r#"{"matches":true}"#)),
            }],
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
