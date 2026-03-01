use crate::css_selector::product_schema::ProductCssSelectorSchema;
use llm::{LLMProvider, chat::ChatMessage, error::LLMError};

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
        let llm = llm
            .resilient(true)
            .resilient_attempts(3)
            .system(
                "You are an e-commerce scraper-assistant for antiques creating extraction-schemas for HTML given product-pages.",
            )
            .openai_enable_web_search(false)
            .reasoning(true)
            .timeout_seconds(120)
            .schema(ProductCssSelectorSchema::structured_output_format())
            .validator(|res| {
                serde_json::from_str::<ProductCssSelectorSchema>(res)
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
            "Generate a robust Extraction-Schema for given HTML. Here is the HTML: \n\n {html}",
        );
        let message = ChatMessage::assistant().content(instruction).build();
        let messages = vec![message];

        let res = self.llm.chat(&messages).await?.text().ok_or_else(|| {
            ProductSchemaServiceError::NoTextResponse("Expected text response".to_string())
        })?;

        serde_json::from_str(&res).map_err(ProductSchemaServiceError::JsonParsingTargetSchemaError)
    }
}
