use crate::{
    ClassificationBatchOptions, ClassificationError, ClassificationOperation,
    ClassificationRequest, ClassificationResponse, ClassificationUsage, ClassifierModel,
    Probability, QuestionId,
};
use base64::{Engine, engine::general_purpose::STANDARD as BASE64};
use futures::StreamExt;
use image_fetcher::{FetchedImage, ImageFetcher};
use reqwest::{StatusCode, header::RETRY_AFTER};
use serde::Serialize;
use serde_json::Value;
use std::{
    collections::{BTreeMap, BTreeSet},
    str::FromStr,
    time::Duration,
};
use url::Url;

const API_BASE: &str = "https://api.cloudflare.com/client/v4/";
const MAX_RESPONSE_BYTES: usize = 1024 * 1024;
const MAX_REQUEST_BYTES: usize = 13 * 1024 * 1024;
const MAX_IMAGE_BYTES: usize = 4 * 1024 * 1024;
const MAX_IMAGE_PIXELS: u64 = 16 * 1024 * 1024;
const MAX_ACCOUNT_ID_BYTES: usize = 128;
const MAX_API_TOKEN_BYTES: usize = 8 * 1024;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CloudflareModel {
    ClefFlash,
    Clef,
}

impl CloudflareModel {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ClefFlash => "clef-flash",
            Self::Clef => "clef",
        }
    }

    fn rest_path(self) -> &'static [&'static str] {
        match self {
            Self::ClefFlash => &["@cf", "cloudflare", "clef-flash"],
            Self::Clef => &["@cf", "cloudflare", "clef"],
        }
    }
}

impl Default for CloudflareModel {
    fn default() -> Self {
        Self::ClefFlash
    }
}

impl FromStr for CloudflareModel {
    type Err = ClassificationError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "clef-flash" => Ok(Self::ClefFlash),
            "clef" => Ok(Self::Clef),
            _ => Err(ClassificationError::InvalidConfiguration),
        }
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct CloudflareClassifierConfig {
    account_id: String,
    api_token: String,
    model: CloudflareModel,
}

impl CloudflareClassifierConfig {
    pub fn new(
        account_id: impl Into<String>,
        api_token: impl Into<String>,
        model: CloudflareModel,
    ) -> Result<Self, ClassificationError> {
        let account_id = account_id.into();
        let api_token = api_token.into();
        if account_id.is_empty()
            || account_id.len() > MAX_ACCOUNT_ID_BYTES
            || !account_id
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
            || api_token.is_empty()
            || api_token.len() > MAX_API_TOKEN_BYTES
            || !api_token.bytes().all(|byte| byte.is_ascii_graphic())
        {
            return Err(ClassificationError::InvalidConfiguration);
        }
        Ok(Self {
            account_id,
            api_token,
            model,
        })
    }

    pub fn model(&self) -> CloudflareModel {
        self.model
    }
}

pub struct CloudflareClassifierModel {
    client: reqwest::Client,
    endpoint: Url,
    api_token: String,
    model: CloudflareModel,
    image_fetcher: ImageFetcher,
}

impl CloudflareClassifierModel {
    pub fn new(config: CloudflareClassifierConfig) -> Result<Self, ClassificationError> {
        let mut endpoint =
            Url::parse(API_BASE).map_err(|_| ClassificationError::InvalidConfiguration)?;
        {
            let mut path = endpoint
                .path_segments_mut()
                .map_err(|_| ClassificationError::InvalidConfiguration)?;
            path.pop_if_empty()
                .push("accounts")
                .push(&config.account_id)
                .push("ai")
                .push("run");
            for segment in config.model.rest_path() {
                path.push(segment);
            }
        }
        Self::with_endpoint(config, endpoint)
    }

    fn with_endpoint(
        config: CloudflareClassifierConfig,
        endpoint: Url,
    ) -> Result<Self, ClassificationError> {
        let client = reqwest::Client::builder()
            .connect_timeout(CONNECT_TIMEOUT)
            .build()
            .map_err(|_| ClassificationError::InvalidConfiguration)?;
        Ok(Self {
            client,
            endpoint,
            api_token: config.api_token,
            model: config.model,
            image_fetcher: ImageFetcher::new(),
        })
    }

    async fn classify_with_images(
        &self,
        request: ClassificationRequest,
        images: Vec<FetchedImage>,
    ) -> Result<ClassificationResponse, ClassificationError> {
        validate_request(&request)?;
        let mut questions = BTreeMap::new();
        for question in request.questions {
            if questions
                .insert(
                    question.id.as_str().to_owned(),
                    CloudflareQuestion {
                        question_type: "noul",
                        instructions: question.instructions,
                    },
                )
                .is_some()
            {
                return Err(ClassificationError::InvalidRequest);
            }
        }
        let expected_question_ids = questions.keys().cloned().collect::<BTreeSet<_>>();

        let images = images
            .into_iter()
            .filter_map(cloudflare_image)
            .map(|image| format!("data:{};base64,{}", image.mime_type(), image.base64_data()))
            .collect::<Vec<_>>();
        let body = CloudflareRequest {
            model: self.model.as_str(),
            state: request.state,
            questions,
            images: (!images.is_empty()).then_some(images),
        };
        let body = serde_json::to_vec(&body).map_err(|_| ClassificationError::InvalidRequest)?;
        if body.len() > MAX_REQUEST_BYTES {
            return Err(ClassificationError::PermanentCandidateFailure);
        }

        let response = self
            .client
            .post(self.endpoint.clone())
            .bearer_auth(&self.api_token)
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .timeout(request.options.request_timeout)
            .body(body)
            .send()
            .await
            .map_err(|error| {
                if error.is_timeout() {
                    ClassificationError::Timeout
                } else {
                    ClassificationError::Transient
                }
            })?;
        let status = response.status();
        let retry_after = retry_after(response.headers().get(RETRY_AFTER));
        let response_bytes = read_bounded_body(response).await;
        if !status.is_success() {
            return Err(status_error(status, retry_after));
        }
        let response_bytes = response_bytes?;
        let envelope: CloudflareEnvelope = serde_json::from_slice(&response_bytes)
            .map_err(|_| ClassificationError::InvalidResponse)?;
        if envelope.success != Some(true) {
            return Err(ClassificationError::InvalidResponse);
        }
        let result = envelope
            .result
            .ok_or(ClassificationError::InvalidResponse)?;
        if result.model != self.model.as_str() {
            return Err(ClassificationError::InvalidResponse);
        }
        parse_answers(result.answers, result.usage, &expected_question_ids)
    }
}

#[async_trait::async_trait]
impl ClassifierModel for CloudflareClassifierModel {
    async fn classify(
        &self,
        request: ClassificationRequest,
    ) -> Result<ClassificationResponse, ClassificationError> {
        validate_request(&request)?;
        let mut images = Vec::with_capacity(1);
        if let Some(url) = request.image_urls.first() {
            if let Some(image) = self.image_fetcher.fetch(url).await {
                images.push(image);
            }
        }
        self.classify_with_images(request, images).await
    }

    async fn classify_batch(
        &self,
        requests: Vec<ClassificationRequest>,
        options: ClassificationBatchOptions,
    ) -> Vec<Result<ClassificationResponse, ClassificationError>> {
        let unique_urls = requests
            .iter()
            .filter(|request| validate_request(request).is_ok())
            .flat_map(|request| request.image_urls.first())
            .map(|url| url.as_str().to_owned())
            .collect::<std::collections::BTreeSet<_>>();
        let image_results = futures::stream::iter(unique_urls)
            .map(|url| async move {
                let parsed = Url::parse(&url).ok()?;
                self.image_fetcher
                    .fetch(&parsed)
                    .await
                    .map(|image| (url, image))
            })
            .buffered(options.max_concurrent_requests.get())
            .filter_map(async move |result| result)
            .collect::<BTreeMap<_, _>>()
            .await;

        futures::stream::iter(requests)
            .map(|request| {
                let images = request
                    .image_urls
                    .first()
                    .and_then(|url| image_results.get(url.as_str()))
                    .cloned()
                    .into_iter()
                    .collect();
                async move { self.classify_with_images(request, images).await }
            })
            .buffered(options.max_concurrent_requests.get())
            .collect()
            .await
    }
}

#[derive(Serialize)]
struct CloudflareRequest {
    model: &'static str,
    state: Value,
    questions: BTreeMap<String, CloudflareQuestion>,
    #[serde(skip_serializing_if = "Option::is_none")]
    images: Option<Vec<String>>,
}

#[derive(Serialize)]
struct CloudflareQuestion {
    #[serde(rename = "type")]
    question_type: &'static str,
    instructions: String,
}

#[derive(serde::Deserialize)]
struct CloudflareEnvelope {
    success: Option<bool>,
    result: Option<CloudflareResult>,
}

#[derive(serde::Deserialize)]
struct CloudflareResult {
    model: String,
    answers: BTreeMap<String, Value>,
    #[serde(default)]
    usage: Option<Value>,
}

fn validate_request(request: &ClassificationRequest) -> Result<(), ClassificationError> {
    if request.operation != ClassificationOperation::ProductEnhancedSearchDescriptionMatching
        || request.questions.is_empty()
        || request.questions.len() > 64
        || request.image_urls.len() > 4
        || request.options.request_timeout.is_zero()
    {
        return Err(ClassificationError::InvalidRequest);
    }
    Ok(())
}

fn cloudflare_image(image: FetchedImage) -> Option<FetchedImage> {
    if !matches!(image.mime_type(), "image/jpeg" | "image/png" | "image/webp") {
        return None;
    }
    let data = BASE64.decode(image.base64_data()).ok()?;
    if data.len() > MAX_IMAGE_BYTES {
        return None;
    }
    let (width, height) = image_dimensions(image.mime_type(), &data)?;
    if u64::from(width).checked_mul(u64::from(height))? > MAX_IMAGE_PIXELS {
        return None;
    }
    Some(image)
}

fn image_dimensions(mime_type: &str, bytes: &[u8]) -> Option<(u32, u32)> {
    match mime_type {
        "image/png" if bytes.starts_with(b"\x89PNG\r\n\x1a\n") => Some((
            u32::from_be_bytes(bytes.get(16..20)?.try_into().ok()?),
            u32::from_be_bytes(bytes.get(20..24)?.try_into().ok()?),
        )),
        "image/jpeg" => jpeg_dimensions(bytes),
        "image/webp" => webp_dimensions(bytes),
        _ => None,
    }
}

fn jpeg_dimensions(bytes: &[u8]) -> Option<(u32, u32)> {
    if !bytes.starts_with(&[0xff, 0xd8]) {
        return None;
    }
    let mut offset = 2;
    while offset < bytes.len() {
        while bytes.get(offset) == Some(&0xff) {
            offset += 1;
        }
        let marker = *bytes.get(offset)?;
        offset += 1;
        if matches!(marker, 0xd8 | 0xd9 | 0x01 | 0xd0..=0xd7) {
            continue;
        }
        let segment_length = usize::from(u16::from_be_bytes([
            *bytes.get(offset)?,
            *bytes.get(offset + 1)?,
        ]));
        if segment_length < 2 {
            return None;
        }
        if matches!(marker, 0xc0..=0xc3 | 0xc5..=0xc7 | 0xc9..=0xcb | 0xcd..=0xcf) {
            let height = u32::from(u16::from_be_bytes([
                *bytes.get(offset + 3)?,
                *bytes.get(offset + 4)?,
            ]));
            let width = u32::from(u16::from_be_bytes([
                *bytes.get(offset + 5)?,
                *bytes.get(offset + 6)?,
            ]));
            return Some((width, height));
        }
        offset = offset.checked_add(segment_length)?;
    }
    None
}

fn webp_dimensions(bytes: &[u8]) -> Option<(u32, u32)> {
    if !bytes.starts_with(b"RIFF") || bytes.get(8..12)? != b"WEBP" {
        return None;
    }
    match bytes.get(12..16)? {
        b"VP8X" => {
            let width =
                u32::from_le_bytes([*bytes.get(24)?, *bytes.get(25)?, *bytes.get(26)?, 0]) + 1;
            let height =
                u32::from_le_bytes([*bytes.get(27)?, *bytes.get(28)?, *bytes.get(29)?, 0]) + 1;
            Some((width, height))
        }
        b"VP8 " if bytes.get(23..26)? == [0x9d, 0x01, 0x2a] => {
            let width = u32::from(u16::from_le_bytes([*bytes.get(26)?, *bytes.get(27)?]) & 0x3fff);
            let height = u32::from(u16::from_le_bytes([*bytes.get(28)?, *bytes.get(29)?]) & 0x3fff);
            Some((width, height))
        }
        b"VP8L" if bytes.get(20) == Some(&0x2f) => {
            let first = u32::from(*bytes.get(21)?);
            let second = u32::from(*bytes.get(22)?);
            let third = u32::from(*bytes.get(23)?);
            let fourth = u32::from(*bytes.get(24)?);
            let width = 1 + first + ((second & 0x3f) << 8);
            let height = 1 + ((second & 0xc0) >> 6) + (third << 2) + ((fourth & 0x0f) << 10);
            Some((width, height))
        }
        _ => None,
    }
}

async fn read_bounded_body(
    mut response: reqwest::Response,
) -> Result<Vec<u8>, ClassificationError> {
    if response
        .content_length()
        .is_some_and(|length| length > MAX_RESPONSE_BYTES as u64)
    {
        return Err(ClassificationError::InvalidResponse);
    }
    let mut body = Vec::with_capacity(
        response
            .content_length()
            .unwrap_or_default()
            .min(MAX_RESPONSE_BYTES as u64) as usize,
    );
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| ClassificationError::Transient)?
    {
        if chunk.len() > MAX_RESPONSE_BYTES.saturating_sub(body.len()) {
            return Err(ClassificationError::InvalidResponse);
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

fn status_error(status: StatusCode, retry_after: Option<Duration>) -> ClassificationError {
    match status.as_u16() {
        401 | 403 => ClassificationError::Authentication,
        429 => ClassificationError::RateLimited { retry_after },
        408 | 425 | 500..=599 => ClassificationError::Transient,
        _ => ClassificationError::InvalidConfiguration,
    }
}

fn retry_after(value: Option<&reqwest::header::HeaderValue>) -> Option<Duration> {
    let value = value?.to_str().ok()?;
    if let Ok(seconds) = value.parse::<u64>() {
        return Some(Duration::from_secs(seconds));
    }
    httpdate::parse_http_date(value).ok().map(|date| {
        date.duration_since(std::time::SystemTime::now())
            .unwrap_or_default()
    })
}

fn parse_answers(
    raw_answers: BTreeMap<String, Value>,
    raw_usage: Option<Value>,
    expected_question_ids: &BTreeSet<String>,
) -> Result<ClassificationResponse, ClassificationError> {
    if raw_answers.len() != expected_question_ids.len()
        || raw_answers
            .keys()
            .any(|question_id| !expected_question_ids.contains(question_id))
    {
        return Err(ClassificationError::InvalidResponse);
    }
    let mut answers = BTreeMap::new();
    for question_id in expected_question_ids {
        let raw = raw_answers
            .get(question_id)
            .and_then(Value::as_object)
            .ok_or(ClassificationError::InvalidResponse)?;
        if raw.len() != 1 {
            return Err(ClassificationError::InvalidResponse);
        }
        let probability = raw
            .get("noul")
            .and_then(Value::as_f64)
            .ok_or(ClassificationError::InvalidResponse)
            .and_then(Probability::new)?;
        let id = QuestionId::new(question_id.clone())?;
        answers.insert(id, probability);
    }
    let usage = ClassificationUsage {
        input_tokens: raw_usage
            .as_ref()
            .and_then(|usage| usage.get("input_tokens"))
            .and_then(Value::as_u64),
        output_tokens: raw_usage
            .as_ref()
            .and_then(|usage| usage.get("output_tokens"))
            .and_then(Value::as_u64),
    };
    Ok(ClassificationResponse { answers, usage })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::BinaryClassificationQuestion;
    use tokio::{io::AsyncReadExt, io::AsyncWriteExt, net::TcpListener};

    fn config(model: CloudflareModel) -> CloudflareClassifierConfig {
        CloudflareClassifierConfig::new("account-id", "secret-test-token", model)
            .unwrap_or_else(|_| panic!("valid Cloudflare test configuration"))
    }

    fn request(image_urls: Vec<Url>) -> ClassificationRequest {
        ClassificationRequest {
            operation: ClassificationOperation::ProductEnhancedSearchDescriptionMatching,
            state: serde_json::json!({
                "user_search": "original search",
                "candidate": {"title": "title", "description": "description"}
            }),
            questions: vec![
                BinaryClassificationQuestion {
                    id: QuestionId::new("hard_conflict")
                        .unwrap_or_else(|_| panic!("valid question id")),
                    instructions: "Does evidence conflict with the user's hard requirements?"
                        .into(),
                },
                BinaryClassificationQuestion {
                    id: QuestionId::new("should_show")
                        .unwrap_or_else(|_| panic!("valid question id")),
                    instructions: "Should this result be shown?".into(),
                },
            ],
            image_urls,
            options: crate::ClassificationOptions::default(),
        }
    }

    async fn serve_once(
        response_body: String,
        status: &str,
    ) -> (Url, tokio::task::JoinHandle<String>) {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .unwrap_or_else(|_| panic!("bind local test server"));
        let address = listener
            .local_addr()
            .unwrap_or_else(|_| panic!("local test address"));
        let body = response_body.clone();
        let status = status.to_owned();
        let task = tokio::spawn(async move {
            let (mut socket, _) = listener
                .accept()
                .await
                .unwrap_or_else(|_| panic!("accept test request"));
            let mut bytes = Vec::new();
            let mut expected_length = None;
            while bytes.len() < 64 * 1024 {
                let mut chunk = [0; 4096];
                let read = socket
                    .read(&mut chunk)
                    .await
                    .unwrap_or_else(|_| panic!("read test request"));
                if read == 0 {
                    break;
                }
                bytes.extend_from_slice(&chunk[..read]);
                if expected_length.is_none()
                    && let Some(header_end) =
                        bytes.windows(4).position(|window| window == b"\r\n\r\n")
                {
                    let headers = String::from_utf8_lossy(&bytes[..header_end]);
                    let content_length = headers.lines().find_map(|line| {
                        let (name, value) = line.split_once(':')?;
                        name.eq_ignore_ascii_case("content-length")
                            .then(|| value.trim().parse::<usize>().ok())
                            .flatten()
                    });
                    expected_length = content_length.map(|length| header_end + 4 + length);
                }
                if expected_length.is_some_and(|length| bytes.len() >= length) {
                    break;
                }
            }
            let request = String::from_utf8_lossy(&bytes).into_owned();
            let response = format!(
                "HTTP/1.1 {status}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            let _ = socket.write_all(response.as_bytes()).await;
            request
        });
        (
            Url::parse(&format!("http://{address}/test"))
                .unwrap_or_else(|_| panic!("valid local endpoint")),
            task,
        )
    }

    #[test]
    fn model_selects_matching_rest_path_and_body_name() {
        let flash = CloudflareClassifierModel::new(config(CloudflareModel::ClefFlash))
            .unwrap_or_else(|_| panic!("valid Cloudflare client"));
        let clef = CloudflareClassifierModel::new(config(CloudflareModel::Clef))
            .unwrap_or_else(|_| panic!("valid Cloudflare client"));
        assert!(
            flash
                .endpoint
                .as_str()
                .ends_with("/@cf/cloudflare/clef-flash")
        );
        assert!(clef.endpoint.as_str().ends_with("/@cf/cloudflare/clef"));
        assert_eq!(CloudflareModel::default(), CloudflareModel::ClefFlash);
        assert_eq!(
            CloudflareModel::from_str("clef").unwrap(),
            CloudflareModel::Clef
        );
        assert!(CloudflareModel::from_str("gemini").is_err());
    }

    #[test]
    fn configuration_rejects_unusable_or_oversized_credentials() {
        assert!(
            CloudflareClassifierConfig::new("account", "token\n", CloudflareModel::Clef).is_err()
        );
        assert!(
            CloudflareClassifierConfig::new(
                "account",
                "x".repeat(MAX_API_TOKEN_BYTES + 1),
                CloudflareModel::Clef,
            )
            .is_err()
        );
        assert!(
            CloudflareClassifierConfig::new(
                "a".repeat(MAX_ACCOUNT_ID_BYTES + 1),
                "token",
                CloudflareModel::Clef,
            )
            .is_err()
        );
    }

    #[tokio::test]
    async fn request_uses_bearer_auth_neutral_state_and_noul_answers() {
        let (endpoint, task) = serve_once(
            r#"{"success":true,"result":{"model":"clef-flash","answers":{"hard_conflict":{"noul":0.1},"should_show":{"noul":0.5}},"usage":{"input_tokens":12,"output_tokens":3}}}"#.into(),
            "200 OK",
        )
        .await;
        let model =
            CloudflareClassifierModel::with_endpoint(config(CloudflareModel::ClefFlash), endpoint)
                .unwrap_or_else(|_| panic!("valid Cloudflare client"));
        let response = model
            .classify(request(Vec::new()))
            .await
            .unwrap_or_else(|_| panic!("valid classification response"));
        let wire_request = task
            .await
            .unwrap_or_else(|_| panic!("server task completed"));

        assert!(
            wire_request
                .to_ascii_lowercase()
                .contains("authorization: bearer secret-test-token")
        );
        assert!(wire_request.contains("\"model\":\"clef-flash\""));
        assert!(wire_request.contains("\"user_search\":\"original search\""));
        assert!(wire_request.contains("\"type\":\"noul\""));
        assert!(!wire_request.contains("\"images\""));
        assert_eq!(
            0.5,
            response.answers[&QuestionId::new("should_show").unwrap()].get()
        );
        assert_eq!(Some(12), response.usage.input_tokens);
    }

    #[tokio::test]
    async fn operational_http_failures_never_become_negative_answers() {
        for (status, expected) in [
            ("401 Unauthorized", ClassificationError::Authentication),
            ("403 Forbidden", ClassificationError::Authentication),
            (
                "429 Too Many Requests",
                ClassificationError::RateLimited { retry_after: None },
            ),
            ("503 Service Unavailable", ClassificationError::Transient),
        ] {
            let (endpoint, task) = serve_once("{}".into(), status).await;
            let model = CloudflareClassifierModel::with_endpoint(
                config(CloudflareModel::ClefFlash),
                endpoint,
            )
            .unwrap_or_else(|_| panic!("valid Cloudflare client"));
            assert_eq!(Err(expected), model.classify(request(Vec::new())).await);
            let _ = task.await;
        }
    }

    #[test]
    fn response_requires_echoed_model_and_all_typed_answers() {
        let expected = BTreeSet::from(["hard_conflict".into(), "should_show".into()]);
        assert!(
            parse_answers(
                BTreeMap::from([
                    ("hard_conflict".into(), serde_json::json!({"noul": 0.1})),
                    ("should_show".into(), serde_json::json!({"noul": 0.7})),
                ]),
                None,
                &expected,
            )
            .is_ok()
        );
        assert_eq!(
            Err(ClassificationError::InvalidResponse),
            parse_answers(
                BTreeMap::from([("hard_conflict".into(), serde_json::json!({"choice": "yes"}))]),
                None,
                &expected,
            )
        );
        assert_eq!(
            Err(ClassificationError::InvalidResponse),
            parse_answers(
                BTreeMap::from([
                    ("hard_conflict".into(), serde_json::json!({"noul": 0.1})),
                    ("should_show".into(), serde_json::json!({"noul": 1.1})),
                ]),
                None,
                &expected,
            )
        );
    }

    #[test]
    fn image_headers_are_limited_to_cloudflare_supported_formats_and_dimensions() {
        let png = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR\0\0\x00\x10\0\0\0\x20";
        assert_eq!(image_dimensions("image/png", png), Some((16, 32)));
        assert_eq!(image_dimensions("image/png", b"GIF89a"), None);
        let too_large = 4096_u32 * 4097_u32;
        assert!(u64::from(too_large) > MAX_IMAGE_PIXELS);
    }
}
