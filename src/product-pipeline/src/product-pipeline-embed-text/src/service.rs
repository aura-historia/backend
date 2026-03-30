use async_trait::async_trait;
use base64::{Engine, engine::general_purpose::STANDARD as BASE64};
use product::core::{description::Description, title::Title};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tracing::{debug, warn};
use url::Url;

#[derive(Debug, Error)]
pub enum MultimodalEmbeddingError {
    #[error("Gemini API request failed: {0}")]
    RequestFailed(#[from] reqwest::Error),
    #[error("Gemini API returned error: {0}")]
    ApiError(String),
    #[error("Empty embedding response from Gemini API")]
    EmptyResponse,
}

#[async_trait]
#[mockall::automock]
pub trait MultimodalEmbeddingService {
    async fn embed(
        &self,
        title: &Title,
        description: Option<&Description>,
        image: Option<&Url>,
    ) -> Result<Vec<f32>, MultimodalEmbeddingError>;
}

pub struct MultimodalEmbeddingServiceImpl {
    api_key: String,
    client: reqwest::Client,
}

impl MultimodalEmbeddingServiceImpl {
    pub fn new(api_key: &str) -> Self {
        Self {
            api_key: api_key.to_string(),
            client: reqwest::Client::new(),
        }
    }

    fn build_content_parts(
        title: &Title,
        description: Option<&Description>,
        image_data: Option<(String, String)>,
    ) -> Vec<ContentPart> {
        let mut parts = Vec::with_capacity(3);

        let text = match description {
            Some(desc) => format!("{title} [SEP] {desc}"),
            None => title.to_string(),
        };
        parts.push(ContentPart::Text { text });

        if let Some((mime_type, data)) = image_data {
            parts.push(ContentPart::InlineData {
                inline_data: InlineData { mime_type, data },
            });
        }

        parts
    }

    async fn fetch_image(&self, url: &Url) -> Option<(String, String)> {
        match self.client.get(url.as_str()).send().await {
            Ok(response) => {
                let content_type = response
                    .headers()
                    .get(reqwest::header::CONTENT_TYPE)
                    .and_then(|v| v.to_str().ok())
                    .unwrap_or("image/jpeg")
                    .to_string();

                let mime_type = content_type
                    .split(';')
                    .next()
                    .unwrap_or("image/jpeg")
                    .trim()
                    .to_string();

                match response.bytes().await {
                    Ok(bytes) => {
                        let encoded = BASE64.encode(&bytes);
                        Some((mime_type, encoded))
                    }
                    Err(err) => {
                        warn!(error = %err, url = %url, "Failed reading image bytes.");
                        None
                    }
                }
            }
            Err(err) => {
                warn!(error = %err, url = %url, "Failed fetching image.");
                None
            }
        }
    }
}

#[async_trait]
impl MultimodalEmbeddingService for MultimodalEmbeddingServiceImpl {
    async fn embed(
        &self,
        title: &Title,
        description: Option<&Description>,
        image: Option<&Url>,
    ) -> Result<Vec<f32>, MultimodalEmbeddingError> {
        let image_data = match image {
            Some(url) => self.fetch_image(url).await,
            None => None,
        };

        let parts = Self::build_content_parts(title, description, image_data);

        let request = EmbedContentRequest {
            model: "models/gemini-embedding-2-preview-03-25",
            content: Content { parts },
        };

        let url = format!(
            "https://generativelanguage.googleapis.com/v1beta/models/gemini-embedding-2-preview-03-25:embedContent?key={}",
            self.api_key
        );

        debug!("Requesting multimodal embedding from Gemini API.");

        let response = self
            .client
            .post(&url)
            .json(&request)
            .send()
            .await?
            .error_for_status()
            .map_err(MultimodalEmbeddingError::RequestFailed)?;

        let body: EmbedContentResponse = response.json().await?;
        let values = body.embedding.values;
        if values.is_empty() {
            return Err(MultimodalEmbeddingError::EmptyResponse);
        }

        Ok(values)
    }
}

#[derive(Debug, Serialize)]
struct EmbedContentRequest<'a> {
    model: &'a str,
    content: Content,
}

#[derive(Debug, Serialize)]
struct Content {
    parts: Vec<ContentPart>,
}

#[derive(Debug, Serialize)]
#[serde(untagged)]
enum ContentPart {
    Text {
        text: String,
    },
    InlineData {
        #[serde(rename = "inlineData")]
        inline_data: InlineData,
    },
}

#[derive(Debug, Serialize)]
struct InlineData {
    #[serde(rename = "mimeType")]
    mime_type: String,
    data: String,
}

#[derive(Debug, Deserialize)]
struct EmbedContentResponse {
    embedding: Embedding,
}

#[derive(Debug, Deserialize)]
struct Embedding {
    values: Vec<f32>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_build_text_only_parts_when_no_description_and_no_image() {
        let title = Title::from("Antique Vase");
        let parts = MultimodalEmbeddingServiceImpl::build_content_parts(&title, None, None);

        assert_eq!(parts.len(), 1);
        match &parts[0] {
            ContentPart::Text { text } => assert_eq!(text, "Antique Vase"),
            _ => panic!("Expected text part"),
        }
    }

    #[test]
    fn should_build_text_with_description_when_description_provided() {
        let title = Title::from("Antique Vase");
        let description = Description::from("A beautiful 18th century vase");
        let parts =
            MultimodalEmbeddingServiceImpl::build_content_parts(&title, Some(&description), None);

        assert_eq!(parts.len(), 1);
        match &parts[0] {
            ContentPart::Text { text } => {
                assert_eq!(text, "Antique Vase [SEP] A beautiful 18th century vase")
            }
            _ => panic!("Expected text part"),
        }
    }

    #[test]
    fn should_build_text_and_image_parts_when_image_data_provided() {
        let title = Title::from("Antique Vase");
        let description = Description::from("Beautiful vase");
        let image_data = Some(("image/jpeg".to_string(), "base64data".to_string()));
        let parts = MultimodalEmbeddingServiceImpl::build_content_parts(
            &title,
            Some(&description),
            image_data,
        );

        assert_eq!(parts.len(), 2);
        match &parts[0] {
            ContentPart::Text { text } => {
                assert_eq!(text, "Antique Vase [SEP] Beautiful vase")
            }
            _ => panic!("Expected text part"),
        }
        match &parts[1] {
            ContentPart::InlineData { inline_data } => {
                assert_eq!(inline_data.mime_type, "image/jpeg");
                assert_eq!(inline_data.data, "base64data");
            }
            _ => panic!("Expected inline data part"),
        }
    }

    #[test]
    fn should_build_title_only_text_when_description_is_none() {
        let title = Title::from("Rare Clock");
        let parts = MultimodalEmbeddingServiceImpl::build_content_parts(
            &title,
            None,
            Some(("image/png".to_string(), "imgdata".to_string())),
        );

        assert_eq!(parts.len(), 2);
        match &parts[0] {
            ContentPart::Text { text } => assert_eq!(text, "Rare Clock"),
            _ => panic!("Expected text part"),
        }
    }

    #[test]
    fn should_serialize_text_part_correctly() {
        let part = ContentPart::Text {
            text: "hello".to_string(),
        };
        let json = serde_json::to_value(&part).unwrap();
        assert_eq!(json, serde_json::json!({"text": "hello"}));
    }

    #[test]
    fn should_serialize_inline_data_part_correctly() {
        let part = ContentPart::InlineData {
            inline_data: InlineData {
                mime_type: "image/jpeg".to_string(),
                data: "abc123".to_string(),
            },
        };
        let json = serde_json::to_value(&part).unwrap();
        assert_eq!(
            json,
            serde_json::json!({"inlineData": {"mimeType": "image/jpeg", "data": "abc123"}})
        );
    }

    #[test]
    fn should_serialize_embed_content_request_correctly() {
        let request = EmbedContentRequest {
            model: "models/gemini-embedding-2-preview-03-25",
            content: Content {
                parts: vec![ContentPart::Text {
                    text: "Test title".to_string(),
                }],
            },
        };
        let json = serde_json::to_value(&request).unwrap();
        assert_eq!(
            json,
            serde_json::json!({
                "model": "models/gemini-embedding-2-preview-03-25",
                "content": {
                    "parts": [{"text": "Test title"}]
                }
            })
        );
    }

    #[test]
    fn should_deserialize_embed_content_response_correctly() {
        let json = r#"{"embedding": {"values": [0.1, 0.2, 0.3]}}"#;
        let response: EmbedContentResponse = serde_json::from_str(json).unwrap();
        assert_eq!(response.embedding.values, vec![0.1, 0.2, 0.3]);
    }
}
