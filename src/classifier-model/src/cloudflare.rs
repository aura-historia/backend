use crate::{
    ClassificationBatchOptions, ClassificationDiagnostics, ClassificationError,
    ClassificationOperation, ClassificationRequest, ClassificationResponse, ClassificationUsage,
    ClassifierModel, ImageOmissionReason, Probability, QuestionId,
};
use base64::{Engine, engine::general_purpose::STANDARD as BASE64};
use futures::StreamExt;
use image_fetcher::{FetchedImage, ImageFetcher};
use reqwest::{StatusCode, header::RETRY_AFTER};
use serde::Serialize;
use serde_json::Value;
use std::{
    collections::{BTreeMap, BTreeSet},
    future::Future,
    num::NonZeroUsize,
    str::FromStr,
    time::{Duration, Instant},
};
use url::Url;

const API_BASE: &str = "https://api.cloudflare.com/client/v4/";
const MAX_RESPONSE_BYTES: usize = 1024 * 1024;
const MAX_REQUEST_BYTES: usize = 13 * 1024 * 1024;
const MAX_IMAGE_BYTES: usize = 4 * 1024 * 1024;
const MAX_IMAGE_PIXELS: u64 = 16 * 1024 * 1024;
const MAX_ACCOUNT_ID_BYTES: usize = 128;
const MAX_API_TOKEN_BYTES: usize = 8 * 1024;
const MAX_QUESTION_INSTRUCTIONS_BYTES: usize = 4 * 1024;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum CloudflareModel {
    #[default]
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
            .redirect(reqwest::redirect::Policy::none())
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
        images: Vec<PreparedCloudflareImage>,
        requested_image_count: usize,
        image_omission_reason: Option<ImageOmissionReason>,
        image_fetch_duration: Duration,
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
            .map(|image| format!("data:{};base64,{}", image.mime_type(), image.base64_data()))
            .collect::<Vec<_>>();
        let prompt_version = prompt_version(&request.state);
        let mut diagnostics = ClassificationDiagnostics {
            provider: "cloudflare".to_owned(),
            model: self.model.as_str().to_owned(),
            requested_image_count,
            sent_image_count: images.len(),
            image_omission_reason,
            ..ClassificationDiagnostics::default()
        };
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

        let request_started = Instant::now();
        let result = async {
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
        .await;
        diagnostics.duration_millis =
            duration_millis(image_fetch_duration.saturating_add(request_started.elapsed()));
        match result {
            Ok(mut response) => {
                response.diagnostics = diagnostics.clone();
                log_classifier_attempt(
                    prompt_version,
                    &diagnostics,
                    Some(&response.usage),
                    Some(&response.answers),
                    None,
                );
                Ok(response)
            }
            Err(error) => {
                log_classifier_attempt(prompt_version, &diagnostics, None, None, Some(&error));
                Err(error)
            }
        }
    }
}

#[async_trait::async_trait]
impl ClassifierModel for CloudflareClassifierModel {
    fn provider_name(&self) -> &'static str {
        "cloudflare"
    }

    fn model_name(&self) -> &'static str {
        self.model.as_str()
    }

    async fn classify(
        &self,
        request: ClassificationRequest,
    ) -> Result<ClassificationResponse, ClassificationError> {
        validate_request(&request)?;
        let requested_image_count = request.image_urls.len();
        let unique_urls = unique_image_urls(std::slice::from_ref(&request));
        let image_results = fetch_unique_images(unique_urls, NonZeroUsize::MIN, |url| async move {
            self.image_fetcher
                .fetch(&url)
                .await
                .ok_or(ImageOmissionReason::FetchFailed)
                .and_then(cloudflare_image)
        })
        .await;
        let prepared = request
            .image_urls
            .first()
            .and_then(|url| image_results.get(url.as_str()))
            .cloned()
            .unwrap_or_default();
        self.classify_with_images(
            request,
            prepared.image.into_iter().collect(),
            requested_image_count,
            prepared.omission_reason,
            prepared.duration,
        )
        .await
    }

    async fn classify_batch(
        &self,
        requests: Vec<ClassificationRequest>,
        options: ClassificationBatchOptions,
    ) -> Vec<Result<ClassificationResponse, ClassificationError>> {
        let unique_urls = unique_image_urls(&requests);
        let image_results = fetch_unique_images(
            unique_urls,
            options.max_concurrent_requests,
            |url| async move {
                self.image_fetcher
                    .fetch(&url)
                    .await
                    .ok_or(ImageOmissionReason::FetchFailed)
                    .and_then(cloudflare_image)
            },
        )
        .await;

        futures::stream::iter(requests)
            .map(|request| {
                let requested_image_count = request.image_urls.len();
                let prepared = request
                    .image_urls
                    .first()
                    .and_then(|url| image_results.get(url.as_str()))
                    .cloned()
                    .unwrap_or_default();
                async move {
                    self.classify_with_images(
                        request,
                        prepared.image.into_iter().collect(),
                        requested_image_count,
                        prepared.omission_reason,
                        prepared.duration,
                    )
                    .await
                }
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
        || request.image_urls.len() > 1
        || request.options.request_timeout.is_zero()
    {
        return Err(ClassificationError::InvalidRequest);
    }
    let mut question_ids = BTreeSet::new();
    for question in &request.questions {
        if question.instructions.trim().is_empty()
            || question.instructions.len() > MAX_QUESTION_INSTRUCTIONS_BYTES
            || !question_ids.insert(question.id.as_str())
        {
            return Err(ClassificationError::InvalidRequest);
        }
    }
    Ok(())
}

#[derive(Debug, Clone, Default)]
struct ImagePreparation {
    image: Option<PreparedCloudflareImage>,
    omission_reason: Option<ImageOmissionReason>,
    duration: Duration,
}

fn unique_image_urls(requests: &[ClassificationRequest]) -> BTreeSet<String> {
    requests
        .iter()
        .filter(|request| validate_request(request).is_ok())
        .flat_map(|request| request.image_urls.first())
        .map(|url| url.as_str().to_owned())
        .collect()
}

async fn fetch_unique_images<F, Fut>(
    urls: BTreeSet<String>,
    max_concurrent_requests: NonZeroUsize,
    fetch: F,
) -> BTreeMap<String, ImagePreparation>
where
    F: Fn(Url) -> Fut + Sync,
    Fut: Future<Output = Result<PreparedCloudflareImage, ImageOmissionReason>> + Send,
{
    futures::stream::iter(urls)
        .map(|url| {
            let fetch = &fetch;
            async move {
                let started = Instant::now();
                let result = match Url::parse(&url) {
                    Ok(url) => fetch(url).await,
                    Err(_) => Err(ImageOmissionReason::FetchFailed),
                };
                let mut preparation = ImagePreparation {
                    duration: started.elapsed(),
                    ..ImagePreparation::default()
                };
                match result {
                    Ok(image) => preparation.image = Some(image),
                    Err(reason) => preparation.omission_reason = Some(reason),
                }
                (url, preparation)
            }
        })
        .buffered(max_concurrent_requests.get())
        .collect()
        .await
}

fn duration_millis(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

fn prompt_version(state: &Value) -> &'static str {
    match state
        .get("evaluation_context")
        .and_then(|context| context.get("prompt_version"))
        .and_then(Value::as_str)
    {
        Some("enhanced-match-v1") => "enhanced-match-v1",
        _ => "unknown",
    }
}

fn log_classifier_attempt(
    prompt_version: &'static str,
    diagnostics: &ClassificationDiagnostics,
    usage: Option<&ClassificationUsage>,
    answers: Option<&BTreeMap<QuestionId, Probability>>,
    error: Option<&ClassificationError>,
) {
    let hard_conflict = answers.and_then(|answers| {
        answers
            .iter()
            .find(|(id, _)| id.as_str() == "hard_conflict")
            .map(|(_, value)| value.get())
    });
    let should_show = answers.and_then(|answers| {
        answers
            .iter()
            .find(|(id, _)| id.as_str() == "should_show")
            .map(|(_, value)| value.get())
    });
    tracing::info!(
        eventType = "CLASSIFIER_INVOCATION",
        classifierProvider = %diagnostics.provider,
        classifierModel = %diagnostics.model,
        promptVersion = prompt_version,
        durationMs = diagnostics.duration_millis,
        requestedImageCount = diagnostics.requested_image_count,
        sentImageCount = diagnostics.sent_image_count,
        imageOmissionReason = diagnostics.image_omission_reason.map(ImageOmissionReason::as_str),
        inputTokens = usage.and_then(|usage| usage.input_tokens),
        outputTokens = usage.and_then(|usage| usage.output_tokens),
        hardConflictScore = hard_conflict,
        shouldShowScore = should_show,
        failureCategory = error.map(ClassificationError::category),
        retryCategory = error.map(ClassificationError::retry_category),
        "Completed classifier invocation."
    );
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PreparedCloudflareImage {
    mime_type: &'static str,
    base64_data: String,
}

impl PreparedCloudflareImage {
    fn mime_type(&self) -> &'static str {
        self.mime_type
    }

    fn base64_data(&self) -> &str {
        &self.base64_data
    }
}

fn cloudflare_image(image: FetchedImage) -> Result<PreparedCloudflareImage, ImageOmissionReason> {
    cloudflare_image_parts(image.mime_type(), image.base64_data())
}

fn cloudflare_image_parts(
    mime_type: &str,
    base64_data: &str,
) -> Result<PreparedCloudflareImage, ImageOmissionReason> {
    if !matches!(mime_type, "image/jpeg" | "image/png" | "image/webp") {
        return Err(ImageOmissionReason::UnsupportedFormat);
    }
    let data = BASE64
        .decode(base64_data)
        .map_err(|_| ImageOmissionReason::InvalidOrOversized)?;
    if data.len() > MAX_IMAGE_BYTES {
        return Err(ImageOmissionReason::InvalidOrOversized);
    }
    let (width, height) =
        image_dimensions(mime_type, &data).ok_or(ImageOmissionReason::InvalidOrOversized)?;
    if u64::from(width)
        .checked_mul(u64::from(height))
        .is_none_or(|pixels| pixels > MAX_IMAGE_PIXELS)
    {
        return Err(ImageOmissionReason::InvalidOrOversized);
    }
    Ok(PreparedCloudflareImage {
        mime_type: match mime_type {
            "image/jpeg" => "image/jpeg",
            "image/png" => "image/png",
            "image/webp" => "image/webp",
            _ => return Err(ImageOmissionReason::UnsupportedFormat),
        },
        base64_data: base64_data.to_owned(),
    })
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
            .ok_or(ClassificationError::InvalidResponse)?;
        let answer: CloudflareNoulAnswer = serde_json::from_value(raw.clone())
            .map_err(|_| ClassificationError::InvalidResponse)?;
        if answer.answer_type != "noul" {
            return Err(ClassificationError::InvalidResponse);
        }
        let id = QuestionId::new(question_id.clone())?;
        answers.insert(id, answer.noul);
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
    Ok(ClassificationResponse {
        answers,
        usage,
        diagnostics: ClassificationDiagnostics::default(),
    })
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct CloudflareNoulAnswer {
    #[serde(rename = "type")]
    answer_type: String,
    noul: Probability,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::BinaryClassificationQuestion;
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::{TcpListener, TcpStream},
    };

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
        serve_once_with_headers(response_body, status, Vec::new()).await
    }

    async fn serve_once_with_headers(
        response_body: String,
        status: &str,
        headers: Vec<(String, String)>,
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
            let request = read_http_request(&mut socket).await;
            let extra_headers = headers
                .iter()
                .map(|(name, value)| format!("{name}: {value}\r\n"))
                .collect::<String>();
            let response = format!(
                "HTTP/1.1 {status}\r\ncontent-type: application/json\r\n{extra_headers}content-length: {}\r\nconnection: close\r\n\r\n{}",
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

    async fn read_http_request(socket: &mut TcpStream) -> String {
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
                && let Some(header_end) = bytes.windows(4).position(|window| window == b"\r\n\r\n")
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
        String::from_utf8_lossy(&bytes).into_owned()
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
            r#"{"success":true,"result":{"model":"clef-flash","answers":{"hard_conflict":{"type":"noul","noul":0.1},"should_show":{"type":"noul","noul":0.5}},"usage":{"input_tokens":12,"output_tokens":3}}}"#.into(),
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
        assert_eq!("cloudflare", response.diagnostics.provider);
        assert_eq!("clef-flash", response.diagnostics.model);
        assert_eq!(0, response.diagnostics.requested_image_count);
        assert_eq!(0, response.diagnostics.sent_image_count);
    }

    #[tokio::test]
    async fn omitted_unsupported_image_falls_back_to_text_only_classification() {
        let image_url = Url::parse("https://example.test/unsupported.gif")
            .unwrap_or_else(|_| panic!("valid candidate image URL"));
        let (endpoint, task) = serve_once(
            r#"{"success":true,"result":{"model":"clef-flash","answers":{"hard_conflict":{"type":"noul","noul":0.1},"should_show":{"type":"noul","noul":0.75}}}}"#.into(),
            "200 OK",
        )
        .await;
        let model =
            CloudflareClassifierModel::with_endpoint(config(CloudflareModel::ClefFlash), endpoint)
                .unwrap_or_else(|_| panic!("valid Cloudflare client"));
        let response = model
            .classify_with_images(
                request(vec![image_url]),
                Vec::new(),
                1,
                Some(ImageOmissionReason::UnsupportedFormat),
                Duration::from_millis(1),
            )
            .await
            .unwrap_or_else(|_| panic!("text-only fallback should classify successfully"));
        let wire_request = task
            .await
            .unwrap_or_else(|_| panic!("server task completed"));

        assert!(!wire_request.contains("\"images\""));
        assert_eq!(1, response.diagnostics.requested_image_count);
        assert_eq!(0, response.diagnostics.sent_image_count);
        assert_eq!(
            Some(ImageOmissionReason::UnsupportedFormat),
            response.diagnostics.image_omission_reason
        );
        assert_eq!(
            0.75,
            response.answers[&QuestionId::new("should_show").unwrap()].get()
        );
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
            ("408 Request Timeout", ClassificationError::Transient),
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

    #[tokio::test]
    async fn retry_after_supports_seconds_and_http_dates() {
        let (endpoint, task) = serve_once_with_headers(
            "{}".into(),
            "429 Too Many Requests",
            vec![("retry-after".into(), "7".into())],
        )
        .await;
        let model =
            CloudflareClassifierModel::with_endpoint(config(CloudflareModel::ClefFlash), endpoint)
                .unwrap_or_else(|_| panic!("valid Cloudflare client"));
        assert_eq!(
            Err(ClassificationError::RateLimited {
                retry_after: Some(Duration::from_secs(7)),
            }),
            model.classify(request(Vec::new())).await
        );
        let _ = task.await;

        let retry_at = std::time::SystemTime::now() + Duration::from_secs(60);
        let (endpoint, task) = serve_once_with_headers(
            "{}".into(),
            "429 Too Many Requests",
            vec![("retry-after".into(), httpdate::fmt_http_date(retry_at))],
        )
        .await;
        let model =
            CloudflareClassifierModel::with_endpoint(config(CloudflareModel::ClefFlash), endpoint)
                .unwrap_or_else(|_| panic!("valid Cloudflare client"));
        let Err(ClassificationError::RateLimited {
            retry_after: Some(retry_after),
        }) = model.classify(request(Vec::new())).await
        else {
            panic!("HTTP-date Retry-After should be parsed");
        };
        assert!(retry_after > Duration::ZERO);
        assert!(retry_after <= Duration::from_secs(60));
        let _ = task.await;
    }

    #[tokio::test]
    async fn bearer_authenticated_client_does_not_follow_redirects() {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .unwrap_or_else(|_| panic!("bind local test server"));
        let address = listener
            .local_addr()
            .unwrap_or_else(|_| panic!("local test address"));
        let server = tokio::spawn(async move {
            let (mut first, _) = listener
                .accept()
                .await
                .unwrap_or_else(|_| panic!("accept first request"));
            let first_request = read_http_request(&mut first).await;
            first
                .write_all(
                    b"HTTP/1.1 302 Found\r\nlocation: /redirect-target\r\ncontent-length: 0\r\nconnection: close\r\n\r\n",
                )
                .await
                .unwrap_or_else(|_| panic!("write redirect"));
            let second_request = match tokio::time::timeout(
                Duration::from_millis(200),
                listener.accept(),
            )
            .await
            {
                Ok(Ok((mut second, _))) => {
                    let request = read_http_request(&mut second).await;
                    let response = format!(
                        "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                        r#"{"success":true,"result":{"model":"clef-flash","answers":{"hard_conflict":{"type":"noul","noul":0.1},"should_show":{"type":"noul","noul":0.7}}}}"#.len(),
                        r#"{"success":true,"result":{"model":"clef-flash","answers":{"hard_conflict":{"type":"noul","noul":0.1},"should_show":{"type":"noul","noul":0.7}}}}"#
                    );
                    let _ = second.write_all(response.as_bytes()).await;
                    Some(request)
                }
                _ => None,
            };
            (first_request, second_request)
        });
        let endpoint = Url::parse(&format!("http://{address}/test"))
            .unwrap_or_else(|_| panic!("valid local endpoint"));
        let model =
            CloudflareClassifierModel::with_endpoint(config(CloudflareModel::ClefFlash), endpoint)
                .unwrap_or_else(|_| panic!("valid Cloudflare client"));
        assert_eq!(
            Err(ClassificationError::InvalidConfiguration),
            model.classify(request(Vec::new())).await
        );
        let (first_request, second_request) = server
            .await
            .unwrap_or_else(|_| panic!("redirect server completed"));
        assert!(
            first_request
                .to_ascii_lowercase()
                .contains("authorization: bearer secret-test-token")
        );
        assert!(
            second_request.is_none(),
            "redirect target must not be requested"
        );
    }

    #[tokio::test]
    async fn timeout_and_connection_failures_are_retryable_errors() {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .unwrap_or_else(|_| panic!("bind local timeout server"));
        let address = listener
            .local_addr()
            .unwrap_or_else(|_| panic!("local timeout server address"));
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener
                .accept()
                .await
                .unwrap_or_else(|_| panic!("accept timeout request"));
            let _ = read_http_request(&mut socket).await;
            tokio::time::sleep(Duration::from_millis(80)).await;
        });
        let endpoint = Url::parse(&format!("http://{address}/timeout"))
            .unwrap_or_else(|_| panic!("valid timeout endpoint"));
        let model =
            CloudflareClassifierModel::with_endpoint(config(CloudflareModel::ClefFlash), endpoint)
                .unwrap_or_else(|_| panic!("valid Cloudflare client"));
        let mut timed_request = request(Vec::new());
        timed_request.options.request_timeout = Duration::from_millis(20);
        assert_eq!(
            Err(ClassificationError::Timeout),
            model.classify(timed_request).await
        );
        let _ = server.await;

        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .unwrap_or_else(|_| panic!("bind local refused port"));
        let address = listener
            .local_addr()
            .unwrap_or_else(|_| panic!("local refused port address"));
        drop(listener);
        let endpoint = Url::parse(&format!("http://{address}/unavailable"))
            .unwrap_or_else(|_| panic!("valid refused endpoint"));
        let model =
            CloudflareClassifierModel::with_endpoint(config(CloudflareModel::ClefFlash), endpoint)
                .unwrap_or_else(|_| panic!("valid Cloudflare client"));
        assert_eq!(
            Err(ClassificationError::Transient),
            model.classify(request(Vec::new())).await
        );
    }

    #[test]
    fn response_requires_echoed_model_and_all_typed_answers() {
        let expected = BTreeSet::from(["hard_conflict".into(), "should_show".into()]);
        assert!(
            parse_answers(
                BTreeMap::from([
                    (
                        "hard_conflict".into(),
                        serde_json::json!({"type": "noul", "noul": 0.1})
                    ),
                    (
                        "should_show".into(),
                        serde_json::json!({"type": "noul", "noul": 0.7})
                    ),
                ]),
                None,
                &expected,
            )
            .is_ok()
        );
        assert_eq!(
            Err(ClassificationError::InvalidResponse),
            parse_answers(
                BTreeMap::from([(
                    "hard_conflict".into(),
                    serde_json::json!({"type": "choice", "choice": "yes"})
                )]),
                None,
                &expected,
            )
        );
        assert_eq!(
            Err(ClassificationError::InvalidResponse),
            parse_answers(
                BTreeMap::from([
                    (
                        "hard_conflict".into(),
                        serde_json::json!({"type": "noul", "noul": 0.1})
                    ),
                    (
                        "should_show".into(),
                        serde_json::json!({"type": "noul", "noul": 1.1})
                    ),
                ]),
                None,
                &expected,
            )
        );
        for invalid_answer in [
            serde_json::json!({"noul": 0.5}),
            serde_json::json!({"type": "choice", "noul": 0.5}),
            serde_json::json!({"type": "noul"}),
            serde_json::json!({"type": "noul", "noul": null}),
            serde_json::json!({"type": "noul", "noul": "0.5"}),
            serde_json::json!({"type": "noul", "noul": 0.5, "choice": "yes"}),
        ] {
            assert_eq!(
                Err(ClassificationError::InvalidResponse),
                parse_answers(
                    BTreeMap::from([
                        ("hard_conflict".into(), invalid_answer),
                        (
                            "should_show".into(),
                            serde_json::json!({"type": "noul", "noul": 0.7}),
                        ),
                    ]),
                    None,
                    &expected,
                )
            );
        }
        assert_eq!(
            Err(ClassificationError::InvalidResponse),
            parse_answers(
                BTreeMap::from([
                    (
                        "hard_conflict".into(),
                        serde_json::json!({"type": "noul", "noul": 0.1}),
                    ),
                    (
                        "unexpected_question".into(),
                        serde_json::json!({"type": "noul", "noul": 0.7}),
                    ),
                ]),
                None,
                &expected,
            )
        );
    }

    #[test]
    fn neutral_request_validation_rejects_bad_questions_and_multiple_images() {
        let mut invalid = request(Vec::new());
        invalid.questions[0].instructions = " \n ".into();
        assert_eq!(
            Err(ClassificationError::InvalidRequest),
            validate_request(&invalid)
        );

        let mut invalid = request(Vec::new());
        invalid.questions[0].instructions = "x".repeat(MAX_QUESTION_INSTRUCTIONS_BYTES + 1);
        assert_eq!(
            Err(ClassificationError::InvalidRequest),
            validate_request(&invalid)
        );

        let mut invalid = request(Vec::new());
        invalid.questions[1].id = invalid.questions[0].id.clone();
        assert_eq!(
            Err(ClassificationError::InvalidRequest),
            validate_request(&invalid)
        );

        let image_urls = vec![
            Url::parse("https://example.test/first.jpg").unwrap(),
            Url::parse("https://example.test/second.jpg").unwrap(),
        ];
        let invalid = request(image_urls);
        assert_eq!(
            Err(ClassificationError::InvalidRequest),
            validate_request(&invalid)
        );
        assert!(unique_image_urls(&[invalid]).is_empty());
    }

    #[tokio::test]
    async fn image_batch_fetch_deduplicates_successful_and_failed_urls() {
        let successful_url = Url::parse("https://example.test/success.png").unwrap();
        let failed_url = Url::parse("https://example.test/failure.png").unwrap();
        let requests = vec![
            request(vec![successful_url.clone()]),
            request(vec![successful_url.clone()]),
            request(vec![failed_url.clone()]),
            request(vec![failed_url.clone()]),
        ];
        let unique_urls = unique_image_urls(&requests);
        assert_eq!(unique_urls.len(), 2);
        let fetch_count = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let images = fetch_unique_images(
            unique_urls,
            NonZeroUsize::new(2).unwrap_or(NonZeroUsize::MIN),
            {
                let fetch_count = std::sync::Arc::clone(&fetch_count);
                move |url| {
                    let fetch_count = std::sync::Arc::clone(&fetch_count);
                    async move {
                        fetch_count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                        if url.path().ends_with("success.png") {
                            Ok(PreparedCloudflareImage {
                                mime_type: "image/png",
                                base64_data: "valid-test-image".to_owned(),
                            })
                        } else {
                            Err(ImageOmissionReason::FetchFailed)
                        }
                    }
                }
            },
        )
        .await;
        assert_eq!(2, fetch_count.load(std::sync::atomic::Ordering::SeqCst));
        assert!(images[successful_url.as_str()].image.is_some());
        assert_eq!(
            Some(ImageOmissionReason::FetchFailed),
            images[failed_url.as_str()].omission_reason
        );
    }

    #[test]
    fn unsupported_image_formats_are_omitted_with_a_bounded_reason() {
        let unsupported = BASE64.encode(b"GIF89a");
        assert_eq!(
            Err(ImageOmissionReason::UnsupportedFormat),
            cloudflare_image_parts("image/gif", &unsupported)
        );
        assert_eq!(
            Err(ImageOmissionReason::InvalidOrOversized),
            cloudflare_image_parts("image/png", "not-base64!")
        );
        let valid_png = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR\0\0\0\x10\0\0\0\x20";
        assert!(cloudflare_image_parts("image/png", &BASE64.encode(valid_png)).is_ok());
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
