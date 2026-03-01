use crate::css_selector::product_schema::ProductCssSelectorSchema;
use kuchiki::traits::*;
use kuchiki::{NodeRef, parse_html};
use llm::{LLMProvider, chat::ChatMessage, error::LLMError};
use schemars::schema_for;

#[derive(Debug, thiserror::Error)]
pub enum ProductSchemaServiceError {
    #[error("LLM error: {0}")]
    LLMError(#[from] LLMError),

    #[error("NoTextResponse: {0}")]
    NoTextResponse(String),

    #[error("JsonParsingTargetSchemaError: {0}")]
    JsonParsingTargetSchemaError(serde_json::Error),
}

#[async_trait::async_trait]
#[mockall::automock]
pub trait ProductSchemaService {
    async fn create_product_schema(
        &self,
        html: &str,
    ) -> Result<ProductCssSelectorSchema, ProductSchemaServiceError>;
}

pub struct ProductSchemaServiceImpl {
    llm: Box<dyn LLMProvider>,
}

impl ProductSchemaServiceImpl {
    pub fn new(llm: llm::builder::LLMBuilder) -> Result<Self, LLMError> {
        let schema = serde_json::to_string_pretty(&schema_for!(ProductCssSelectorSchema))
            .unwrap_or_else(|_| "Failed to generate schema".to_string());
        let system_prompt = format!(
            "You are an e-commerce scraper-assistant for antiques creating extraction-schemas for HTML given product-pages.
            Only answer with JSON for the following schema: \n\n {schema}",
        );
        let llm = llm
            .resilient(true)
            .resilient_attempts(3)
            .system(system_prompt)
            .openai_enable_web_search(false)
            .reasoning(true)
            .timeout_seconds(180)
            .validator(|res| {
                serde_json::from_str::<ProductCssSelectorSchema>(strip_markdown_json_embedding(res))
                    .map(|_| ())
                    .map_err(|err| err.to_string())
            })
            .validator_attempts(3)
            .build()?;
        Ok(Self { llm })
    }
}

#[async_trait::async_trait]
impl ProductSchemaService for ProductSchemaServiceImpl {
    async fn create_product_schema(
        &self,
        html: &str,
    ) -> Result<ProductCssSelectorSchema, ProductSchemaServiceError> {
        let instruction = format!(
            "Generate a robust Extraction-Schema for given HTML. Here is the HTML: \n\n {}",
            clean_html_for_schema_generation(html),
        );
        let message = ChatMessage::user().content(instruction).build();
        let messages = vec![message];

        let res = self.llm.chat(&messages).await?.text().ok_or_else(|| {
            ProductSchemaServiceError::NoTextResponse("Expected text response".to_string())
        })?;

        serde_json::from_str(strip_markdown_json_embedding(&res))
            .map_err(ProductSchemaServiceError::JsonParsingTargetSchemaError)
    }
}

pub fn strip_markdown_json_embedding(s: &str) -> &str {
    s.trim()
        .strip_prefix("```json")
        .unwrap_or(s)
        .strip_suffix("```")
        .unwrap_or(s)
}

pub fn clean_html_for_schema_generation(input: &str) -> String {
    let document = parse_html().one(input);

    // Tags to remove entirely
    let remove_selectors = [
        "script", "style", "noscript", "svg", "canvas", "header", "footer", "nav", "aside",
    ];

    for selector in &remove_selectors {
        if let Ok(nodes) = document.select(selector) {
            for node in nodes {
                node.as_node().detach();
            }
        }
    }

    // Remove HTML comments
    remove_comments(&document);

    // Strip unnecessary attributes (keep id + class)
    strip_attributes(&document);

    // Serialize cleaned HTML
    let mut cleaned = Vec::new();
    document.serialize(&mut cleaned).unwrap();
    String::from_utf8(cleaned).unwrap_or_default()
}

fn remove_comments(node: &NodeRef) {
    for child in node.children() {
        if child.as_comment().is_some() {
            child.detach();
        } else {
            remove_comments(&child);
        }
    }
}

fn strip_attributes(document: &NodeRef) {
    let deny_prefixes = ["on"]; // onclick, onload, etc.

    let deny_exact = [
        "style",
        "integrity",
        "crossorigin",
        "referrerpolicy",
        "nonce",
        "tabindex",
        "width",
        "height",
        "loading",
        "decoding",
    ];

    for css_match in document.select("*").unwrap() {
        let mut attributes = css_match.attributes.borrow_mut();

        attributes.map.retain(|key, _| {
            let name = key.local.as_ref();

            // Remove JS event handlers
            if deny_prefixes.iter().any(|prefix| name.starts_with(prefix)) {
                return false;
            }

            // Remove known useless attributes
            if deny_exact.contains(&name) {
                return false;
            }

            true
        });
    }
}

#[cfg(test)]
mod tests {
    use crate::{
        css_selector::product_schema_service::{
            ProductSchemaService, ProductSchemaServiceImpl, clean_html_for_schema_generation,
        },
        normalization::product_normalization_service::{
            ProductNormalizationService, ProductNormalizationServiceImpl,
        },
    };
    use llm::builder::{LLMBackend, LLMBuilder};
    use scraper::Html;
    use url::Url;

    #[tokio::test]
    #[ignore]
    async fn test_create_product_schema() {
        let llm_builder = LLMBuilder::new()
            .backend(LLMBackend::Google)
            .api_key("foo")
            .model("gemini-2.5-flash")
            .max_tokens(8192);
        let service = ProductSchemaServiceImpl::new(llm_builder).unwrap();

        let html = reqwest::Client::new()
            .get("https://www.antiquitaeten-tuebingen.de/augsburger-hinterglasbild-josef-mit-jesuskind-und-lilie-g1500/")
            .send()
            .await
            .unwrap()
            .text()
            .await
            .unwrap();
        println!("Init HTML-Len: '{}'", html.len());

        let cleaned_html = clean_html_for_schema_generation(&html);
        println!("Cleaned HTML-Len: '{}'", cleaned_html.len());

        let schema = service.create_product_schema(&cleaned_html).await.unwrap();
        println!("{:#?} \n\n\n", schema);

        let applied = schema.apply(&Html::parse_document(&html)).unwrap();
        println!("{:#?}", applied);

        let normalized = ProductNormalizationServiceImpl::new()
            .normalize(
                applied,
                Url::parse(
                    "https://www.antiquitaeten-tuebingen.de/augsburger-hinterglasbild-josef-mit-jesuskind-und-lilie-g1500/",
                )
                .unwrap(),
            )
            .unwrap();
        println!("{:#?}", normalized);
    }
}
