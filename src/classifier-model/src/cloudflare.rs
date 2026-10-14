use crate::{
    ClassificationDiagnostics, ClassificationError, ClassificationOperation, ClassificationRequest,
    ClassificationRequestLimits, ClassificationResponse, ClassificationUsage, ClassifierModel,
    ImageOmissionReason, Probability, QuestionId,
};
use base64::{Engine, engine::general_purpose::STANDARD as BASE64};
use futures::StreamExt;
use image::{ImageDecoder, ImageFormat, ImageReader, Limits};
use image_fetcher::{FetchedImage, ImageFetcher};
use reqwest::{StatusCode, header::RETRY_AFTER};
use serde::Serialize;
use serde_json::Value;
use std::{
    collections::{BTreeMap, BTreeSet},
    future::Future,
    io::Cursor,
    num::NonZeroUsize,
    str::FromStr,
    time::{Duration, Instant},
};
use url::Url;

const API_BASE: &str = "https://api.cloudflare.com/client/v4/";
const MAX_RESPONSE_BYTES: usize = 1024 * 1024;
const MAX_REQUEST_BYTES: usize = 13 * 1024 * 1024;
const MAX_IMAGE_BYTES: usize = 4 * 1024 * 1024;
const MAX_TOTAL_IMAGE_BYTES: usize = 8 * 1024 * 1024;
const MAX_IMAGES: usize = 4;
const IMAGE_CONTEXT_TOKENS: usize = 1027;
const CONTEXT_SCHEMA_RESERVE: usize = 1024;
const MAX_IMAGE_PIXELS: u64 = 16 * 1024 * 1024;
const MAX_DECODED_IMAGE_BYTES: u64 = 64 * 1024 * 1024;
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
    fn context_tokens(self) -> usize {
        match self {
            Self::ClefFlash => 24_576,
            Self::Clef => 65_536,
        }
    }

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
        validate_request(&request, self.model)?;
        let operation = request.operation.as_str();
        let correlation_id = request.correlation_id.clone();
        let allow_partial_answers = request.options.allow_partial_answers;
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

        if images.len() > MAX_IMAGES
            || images
                .iter()
                .map(PreparedCloudflareImage::decoded_bytes)
                .sum::<usize>()
                > MAX_TOTAL_IMAGE_BYTES
        {
            return Err(ClassificationError::PermanentCandidateFailure);
        }
        let images = images
            .into_iter()
            .map(|image| format!("data:{};base64,{}", image.mime_type(), image.base64_data()))
            .collect::<Vec<_>>();
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
            // Value's default object decoder silently overwrites duplicate keys.
            let envelope: UniqueValue = serde_json::from_slice(&response_bytes)
                .map_err(|_| ClassificationError::InvalidResponse)?;
            let envelope: CloudflareEnvelope = serde_json::from_value(envelope.0)
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
            parse_answers_with_partial(
                result.answers,
                result.usage,
                &expected_question_ids,
                allow_partial_answers,
            )
        }
        .await;
        diagnostics.duration_millis =
            duration_millis(image_fetch_duration.saturating_add(request_started.elapsed()));
        match result {
            Ok(mut response) => {
                response.diagnostics = diagnostics.clone();
                log_classifier_attempt(
                    operation,
                    correlation_id.as_deref(),
                    &diagnostics,
                    Some(&response.usage),
                    None,
                );
                Ok(response)
            }
            Err(error) => {
                log_classifier_attempt(
                    operation,
                    correlation_id.as_deref(),
                    &diagnostics,
                    None,
                    Some(&error),
                );
                Err(error)
            }
        }
    }
}

#[async_trait::async_trait]
impl ClassifierModel for CloudflareClassifierModel {
    fn request_limits(&self) -> Option<ClassificationRequestLimits> {
        Some(request_limits(self.model))
    }
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
        validate_request(&request, self.model)?;
        let requested_image_count = request.image_urls.len();
        let unique_urls = unique_image_urls(std::slice::from_ref(&request), self.model);
        let image_results = fetch_unique_images(unique_urls, NonZeroUsize::MIN, |url| async move {
            self.image_fetcher
                .fetch(&url)
                .await
                .ok_or(ImageOmissionReason::FetchFailed)
                .and_then(cloudflare_image)
        })
        .await;
        let (images, omission_reason, duration) = prepare_request_images(&request, &image_results);
        self.classify_with_images(
            request,
            images,
            requested_image_count,
            omission_reason,
            duration,
        )
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

fn validate_request(
    request: &ClassificationRequest,
    model: CloudflareModel,
) -> Result<(), ClassificationError> {
    if request.operation != ClassificationOperation::ProductEnhancedSearchDescriptionMatching
        || request.correlation_id.as_ref().is_some_and(|id| {
            id.len() > 160
                || !id
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"_:-.".contains(&b))
        })
        || request.questions.is_empty()
        || request.questions.len() > 64
        || request.image_urls.len() > MAX_IMAGES
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
    if !request_limits(model).permits(request) {
        return Err(ClassificationError::PermanentCandidateFailure);
    }
    Ok(())
}

fn request_limits(model: CloudflareModel) -> ClassificationRequestLimits {
    ClassificationRequestLimits {
        max_questions: 64,
        max_images: MAX_IMAGES,
        max_text_tokens: model.context_tokens() - CONTEXT_SCHEMA_RESERVE,
        max_context_tokens: model.context_tokens(),
        context_reserve_tokens: CONTEXT_SCHEMA_RESERVE,
        tokens_per_image: IMAGE_CONTEXT_TOKENS,
        max_payload_bytes: MAX_REQUEST_BYTES,
    }
}

#[derive(Debug, Clone, Default)]
struct ImagePreparation {
    image: Option<PreparedCloudflareImage>,
    omission_reason: Option<ImageOmissionReason>,
    duration: Duration,
}

fn unique_image_urls(
    requests: &[ClassificationRequest],
    model: CloudflareModel,
) -> BTreeSet<String> {
    requests
        .iter()
        .filter(|request| validate_request(request, model).is_ok())
        .flat_map(|request| request.image_urls.iter())
        .map(|url| url.as_str().to_owned())
        .collect()
}

fn prepare_request_images(
    request: &ClassificationRequest,
    results: &BTreeMap<String, ImagePreparation>,
) -> (
    Vec<PreparedCloudflareImage>,
    Option<ImageOmissionReason>,
    Duration,
) {
    let mut images = Vec::new();
    let mut omission = None;
    let mut duration = Duration::ZERO;
    let mut total_bytes = 0;
    let mut seen_urls = BTreeSet::new();
    for (index, url) in request.image_urls.iter().enumerate() {
        if !seen_urls.insert(url.as_str()) {
            log_image_omission(request.correlation_id.as_deref(), index, "duplicate_url");
            continue;
        }
        let Some(prepared) = results.get(url.as_str()) else {
            omission.get_or_insert(ImageOmissionReason::FetchFailed);
            log_image_omission(
                request.correlation_id.as_deref(),
                index,
                ImageOmissionReason::FetchFailed.as_str(),
            );
            continue;
        };
        duration = duration.saturating_add(prepared.duration);
        if let Some(image) = &prepared.image {
            if images.contains(image) {
                log_image_omission(request.correlation_id.as_deref(), index, "duplicate_image");
                continue;
            }
            let bytes = image.decoded_bytes();
            if total_bytes + bytes > MAX_TOTAL_IMAGE_BYTES {
                omission.get_or_insert(ImageOmissionReason::AggregateLimit);
                log_image_omission(
                    request.correlation_id.as_deref(),
                    index,
                    ImageOmissionReason::AggregateLimit.as_str(),
                );
                continue;
            }
            total_bytes += bytes;
            images.push(image.clone());
        } else if let Some(reason) = prepared.omission_reason {
            omission.get_or_insert(reason);
            log_image_omission(request.correlation_id.as_deref(), index, reason.as_str());
        }
    }
    (images, omission, duration)
}

fn log_image_omission(correlation_id: Option<&str>, index: usize, reason: &'static str) {
    tracing::info!(
        eventType = "CLASSIFIER_IMAGE_OMITTED",
        correlationId = correlation_id,
        requestedImageIndex = index,
        imageOmissionReason = reason,
        "Classifier image omitted."
    );
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

fn log_classifier_attempt(
    operation: &'static str,
    correlation_id: Option<&str>,
    diagnostics: &ClassificationDiagnostics,
    usage: Option<&ClassificationUsage>,
    error: Option<&ClassificationError>,
) {
    tracing::info!(
        eventType = "CLASSIFIER_INVOCATION",
        classifierOperation = operation,
        correlationId = correlation_id,
        classifierProvider = %diagnostics.provider,
        classifierModel = %diagnostics.model,
        durationMs = diagnostics.duration_millis,
        requestedImageCount = diagnostics.requested_image_count,
        sentImageCount = diagnostics.sent_image_count,
        imageOmissionReason = diagnostics.image_omission_reason.map(ImageOmissionReason::as_str),
        inputTokens = usage.and_then(|usage| usage.input_tokens),
        outputTokens = usage.and_then(|usage| usage.output_tokens),
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
    fn decoded_bytes(&self) -> usize {
        self.base64_data.len() / 4 * 3
            - self
                .base64_data
                .bytes()
                .rev()
                .take_while(|byte| *byte == b'=')
                .count()
    }
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
    if !image_is_decodable(mime_type, &data) {
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

fn image_is_decodable(mime_type: &str, bytes: &[u8]) -> bool {
    let format = match mime_type {
        "image/jpeg" => ImageFormat::Jpeg,
        "image/png" => ImageFormat::Png,
        "image/webp" => ImageFormat::WebP,
        _ => return false,
    };
    let Ok(mut decoder) = ImageReader::with_format(Cursor::new(bytes), format).into_decoder()
    else {
        return false;
    };
    let (width, height) = decoder.dimensions();
    if u64::from(width)
        .checked_mul(u64::from(height))
        .is_none_or(|pixels| pixels == 0 || pixels > MAX_IMAGE_PIXELS)
    {
        return false;
    }
    let total_bytes = decoder.total_bytes();
    if total_bytes > MAX_DECODED_IMAGE_BYTES {
        return false;
    }
    let mut limits = Limits::default();
    limits.max_alloc = Some(MAX_DECODED_IMAGE_BYTES);
    if decoder.set_limits(limits).is_err() {
        return false;
    }
    let Ok(output_len) = usize::try_from(total_bytes) else {
        return false;
    };
    let mut output = Vec::new();
    if output.try_reserve_exact(output_len).is_err() {
        return false;
    }
    output.resize(output_len, 0);
    decoder.read_image(&mut output).is_ok()
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

#[cfg(test)]
fn parse_answers(
    raw_answers: BTreeMap<String, Value>,
    raw_usage: Option<Value>,
    expected_question_ids: &BTreeSet<String>,
) -> Result<ClassificationResponse, ClassificationError> {
    parse_answers_with_partial(raw_answers, raw_usage, expected_question_ids, false)
}

fn parse_answers_with_partial(
    raw_answers: BTreeMap<String, Value>,
    raw_usage: Option<Value>,
    expected_question_ids: &BTreeSet<String>,
    allow_partial: bool,
) -> Result<ClassificationResponse, ClassificationError> {
    if (!allow_partial && raw_answers.len() != expected_question_ids.len())
        || raw_answers
            .keys()
            .any(|question_id| !expected_question_ids.contains(question_id))
    {
        return Err(ClassificationError::InvalidResponse);
    }
    let mut answers = BTreeMap::new();
    for question_id in expected_question_ids {
        let answer = raw_answers
            .get(question_id)
            .and_then(|raw| serde_json::from_value::<CloudflareNoulAnswer>(raw.clone()).ok())
            .filter(|answer| answer.answer_type == "noul");
        let Some(answer) = answer else {
            if allow_partial {
                continue;
            }
            return Err(ClassificationError::InvalidResponse);
        };
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

/// Reject duplicate keys at every depth, including inside an individual answer.
struct UniqueValue(Value);

impl<'de> serde::Deserialize<'de> for UniqueValue {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Visitor;
        impl<'de> serde::de::Visitor<'de> for Visitor {
            type Value = UniqueValue;
            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("JSON without duplicate object keys")
            }
            fn visit_map<A: serde::de::MapAccess<'de>>(
                self,
                mut map: A,
            ) -> Result<Self::Value, A::Error> {
                let mut values = serde_json::Map::new();
                while let Some((key, value)) = map.next_entry::<String, UniqueValue>()? {
                    if values.insert(key, value.0).is_some() {
                        return Err(serde::de::Error::custom("duplicate object key"));
                    }
                }
                Ok(UniqueValue(Value::Object(values)))
            }
            fn visit_seq<A: serde::de::SeqAccess<'de>>(
                self,
                mut seq: A,
            ) -> Result<Self::Value, A::Error> {
                let mut values = Vec::new();
                while let Some(value) = seq.next_element::<UniqueValue>()? {
                    values.push(value.0);
                }
                Ok(UniqueValue(Value::Array(values)))
            }
            fn visit_bool<E: serde::de::Error>(self, value: bool) -> Result<Self::Value, E> {
                Ok(UniqueValue(value.into()))
            }
            fn visit_i64<E: serde::de::Error>(self, value: i64) -> Result<Self::Value, E> {
                Ok(UniqueValue(value.into()))
            }
            fn visit_u64<E: serde::de::Error>(self, value: u64) -> Result<Self::Value, E> {
                Ok(UniqueValue(value.into()))
            }
            fn visit_f64<E: serde::de::Error>(self, value: f64) -> Result<Self::Value, E> {
                serde_json::Number::from_f64(value)
                    .map(|number| UniqueValue(Value::Number(number)))
                    .ok_or_else(|| E::custom("invalid number"))
            }
            fn visit_str<E: serde::de::Error>(self, value: &str) -> Result<Self::Value, E> {
                Ok(UniqueValue(value.into()))
            }
            fn visit_string<E: serde::de::Error>(self, value: String) -> Result<Self::Value, E> {
                Ok(UniqueValue(value.into()))
            }
            fn visit_unit<E: serde::de::Error>(self) -> Result<Self::Value, E> {
                Ok(UniqueValue(Value::Null))
            }
        }
        deserializer.deserialize_any(Visitor)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{BinaryClassificationQuestion, ClassificationBatchOptions};
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::{TcpListener, TcpStream},
    };

    #[test]
    fn configured_model_budgets_match_adapter_admission_without_truncating_state() {
        let mut request = request(Vec::new());
        request.state = serde_json::json!({"criteria": "x".repeat(30_000)});
        let original = request.state.clone();
        for (model, context_tokens, admitted) in [
            (CloudflareModel::ClefFlash, 24_576, false),
            (CloudflareModel::Clef, 65_536, true),
        ] {
            let classifier = CloudflareClassifierModel::new(config(model)).unwrap();
            let limits = classifier.request_limits().unwrap();
            assert_eq!(context_tokens, limits.max_context_tokens);
            assert_eq!(admitted, limits.permits(&request));
            assert_eq!(
                if admitted {
                    Ok(())
                } else {
                    Err(ClassificationError::PermanentCandidateFailure)
                },
                validate_request(&request, model)
            );
            assert_eq!(original, request.state);
        }
    }

    fn shared_text_request() -> ClassificationRequest {
        let mut request = request(Vec::new());
        request.state = serde_json::json!({
            "search_intent": { "base_product_queries": ["antique lamp"] },
            "candidates": [
                {"id": "c0000", "evidence": {"title": "Brass lamp"}},
                {"id": "c0001", "evidence": {"title": "Modern reproduction"}}
            ]
        });
        let questions = request.questions.clone();
        request.questions = (0..2)
            .flat_map(|index| {
                questions.iter().map(move |question| {
                    let mut question = question.clone();
                    question.id =
                        QuestionId::new(format!("c{index:04}.{}", question.id.as_str())).unwrap();
                    question
                })
            })
            .collect();
        request.options.allow_partial_answers = true;
        request
    }

    #[tokio::test]
    async fn shared_wire_request_preserves_reordered_keyed_and_independently_valid_answers() {
        let (endpoint, task) = serve_once(
            r#"{"success":true,"result":{"model":"clef-flash","answers":{"c0001.should_show":{"type":"noul","noul":1.1},"c0000.should_show":{"type":"noul","noul":0.8},"c0001.hard_conflict":{"type":"noul","noul":0.9},"c0000.hard_conflict":{"type":"noul","noul":0.1}},"usage":{"input_tokens":123}}}"#.into(), "200 OK"
        ).await;
        let model =
            CloudflareClassifierModel::with_endpoint(config(CloudflareModel::ClefFlash), endpoint)
                .unwrap();
        let response = model.classify(shared_text_request()).await.unwrap();
        assert_eq!(3, response.answers.len());
        assert_eq!(
            0.8,
            response.answers[&QuestionId::new("c0000.should_show").unwrap()].get()
        );
        assert!(
            !response
                .answers
                .contains_key(&QuestionId::new("c0001.should_show").unwrap())
        );
        assert_eq!(Some(123), response.usage.input_tokens);
        let wire = task.await.unwrap();
        let body: Value = serde_json::from_str(wire.split_once("\r\n\r\n").unwrap().1).unwrap();
        assert_eq!(2, body["state"]["candidates"].as_array().unwrap().len());
        assert_eq!(4, body["questions"].as_object().unwrap().len());
        assert!(body.get("images").is_none());
    }

    #[tokio::test]
    async fn duplicate_and_unknown_provider_ids_invalidate_shared_mapping() {
        for answers in [
            r#""c0000.should_show":{"type":"noul","noul":0.1},"c0000.should_show":{"type":"noul","noul":0.9}"#,
            r#""c0000.should_show":{"type":"noul","noul":0.1,"noul":0.9}"#,
            r#""unknown.should_show":{"type":"noul","noul":0.9}"#,
        ] {
            let (endpoint, task) = serve_once(format!(r#"{{"success":true,"result":{{"model":"clef-flash","answers":{{{answers}}}}}}}"#), "200 OK").await;
            let model = CloudflareClassifierModel::with_endpoint(
                config(CloudflareModel::ClefFlash),
                endpoint,
            )
            .unwrap();
            assert_eq!(
                Err(ClassificationError::InvalidResponse),
                model.classify(shared_text_request()).await
            );
            task.await.unwrap();
        }
    }

    #[test]
    fn partial_answers_validate_missing_malformed_and_out_of_range_values_independently() {
        let expected = shared_text_request()
            .questions
            .iter()
            .map(|q| q.id.as_str().to_owned())
            .collect();
        for invalid in [
            serde_json::json!({"type": "noul", "noul": null}),
            serde_json::json!({"type": "noul", "noul": -0.01}),
            serde_json::json!({"type": "noul", "noul": 1.01}),
            serde_json::json!({"type": "choice", "noul": 0.8}),
            serde_json::json!({"type": "noul", "noul": "0.8"}),
            serde_json::json!([0.8]),
        ] {
            let response = parse_answers_with_partial(
                BTreeMap::from([
                    (
                        "c0000.should_show".into(),
                        serde_json::json!({"type": "noul", "noul": 0.8}),
                    ),
                    ("c0001.should_show".into(), invalid),
                ]),
                None,
                &expected,
                true,
            )
            .unwrap();
            assert_eq!(1, response.answers.len());
            assert_eq!(
                0.8,
                response.answers[&QuestionId::new("c0000.should_show").unwrap()].get()
            );
        }
    }

    fn config(model: CloudflareModel) -> CloudflareClassifierConfig {
        CloudflareClassifierConfig::new("account-id", "secret-test-token", model)
            .unwrap_or_else(|_| panic!("valid Cloudflare test configuration"))
    }

    fn request(image_urls: Vec<Url>) -> ClassificationRequest {
        ClassificationRequest {
            correlation_id: None,
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

    fn encoded_test_image(format: ImageFormat) -> Vec<u8> {
        let mut output = Cursor::new(Vec::new());
        image::DynamicImage::new_rgb8(1, 1)
            .write_to(&mut output, format)
            .unwrap_or_else(|_| panic!("encode valid test image"));
        output.into_inner()
    }

    async fn assert_invalid_image_falls_back_to_text_only(mime_type: &str, bytes: &[u8]) {
        let omission_reason = cloudflare_image_parts(mime_type, &BASE64.encode(bytes))
            .expect_err("invalid image must be omitted");
        assert_eq!(ImageOmissionReason::InvalidOrOversized, omission_reason);

        let (endpoint, task) = serve_once(
            r#"{"success":true,"result":{"model":"clef-flash","answers":{"hard_conflict":{"type":"noul","noul":0.1},"should_show":{"type":"noul","noul":0.75}}}}"#.into(),
            "200 OK",
        )
        .await;
        let model =
            CloudflareClassifierModel::with_endpoint(config(CloudflareModel::ClefFlash), endpoint)
                .unwrap_or_else(|_| panic!("valid Cloudflare client"));
        let image_url = Url::parse("https://example.test/listing-image")
            .unwrap_or_else(|_| panic!("valid candidate image URL"));
        let response = model
            .classify_with_images(
                request(vec![image_url]),
                Vec::new(),
                1,
                Some(omission_reason),
                Duration::ZERO,
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
            Some(omission_reason),
            response.diagnostics.image_omission_reason
        );
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
    fn neutral_request_validation_rejects_bad_questions_and_more_than_four_images() {
        let mut invalid = request(Vec::new());
        invalid.questions[0].instructions = " \n ".into();
        assert_eq!(
            Err(ClassificationError::InvalidRequest),
            validate_request(&invalid, CloudflareModel::ClefFlash)
        );

        let mut invalid = request(Vec::new());
        invalid.questions[0].instructions = "x".repeat(MAX_QUESTION_INSTRUCTIONS_BYTES + 1);
        assert_eq!(
            Err(ClassificationError::InvalidRequest),
            validate_request(&invalid, CloudflareModel::ClefFlash)
        );

        let mut invalid = request(Vec::new());
        invalid.questions[1].id = invalid.questions[0].id.clone();
        assert_eq!(
            Err(ClassificationError::InvalidRequest),
            validate_request(&invalid, CloudflareModel::ClefFlash)
        );

        let image_urls = (0..5)
            .map(|index| Url::parse(&format!("https://example.test/{index}.jpg")).unwrap())
            .collect();
        let invalid = request(image_urls);
        assert_eq!(
            Err(ClassificationError::InvalidRequest),
            validate_request(&invalid, CloudflareModel::ClefFlash)
        );
        assert!(unique_image_urls(&[invalid], CloudflareModel::ClefFlash).is_empty());
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
        let unique_urls = unique_image_urls(&requests, CloudflareModel::ClefFlash);
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
    }

    #[test]
    fn fully_decoded_supported_images_are_accepted() {
        for (mime_type, format) in [
            ("image/jpeg", ImageFormat::Jpeg),
            ("image/png", ImageFormat::Png),
            ("image/webp", ImageFormat::WebP),
        ] {
            assert!(
                cloudflare_image_parts(mime_type, &BASE64.encode(encoded_test_image(format)))
                    .is_ok()
            );
        }
    }

    #[tokio::test]
    async fn truncated_images_are_omitted_and_classified_without_image_evidence() {
        let truncated_png = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR\0\0\0\x10\0\0\0\x20";
        let truncated_jpeg =
            b"\xff\xd8\xff\xc0\0\x11\x08\0\x01\0\x01\x03\x01\x11\0\x02\x11\0\x03\x11\0";
        let truncated_webp = b"RIFF\x16\0\0\0WEBPVP8X\x0a\0\0\0\0\0\0\0\0\0\0\0\0\0";
        let truncated_images = [
            ("image/png", truncated_png.as_slice()),
            ("image/jpeg", truncated_jpeg.as_slice()),
            ("image/webp", truncated_webp.as_slice()),
        ];

        for (mime_type, bytes) in truncated_images {
            assert_invalid_image_falls_back_to_text_only(mime_type, bytes).await;
        }
    }

    #[tokio::test]
    async fn corrupt_images_are_omitted_and_classified_without_image_evidence() {
        let mut corrupt_jpeg = encoded_test_image(ImageFormat::Jpeg);
        let sof_offset = corrupt_jpeg
            .windows(2)
            .position(|marker| marker == [0xff, 0xc0])
            .unwrap_or_else(|| panic!("test JPEG has a baseline frame header"));
        corrupt_jpeg[sof_offset + 5] = 0;
        corrupt_jpeg[sof_offset + 6] = 0;
        let mut corrupt_png = encoded_test_image(ImageFormat::Png);
        let idat_offset = corrupt_png
            .windows(4)
            .position(|chunk| chunk == b"IDAT")
            .unwrap_or_else(|| panic!("test PNG has an IDAT chunk"));
        let idat_length = u32::from_be_bytes(
            corrupt_png[idat_offset - 4..idat_offset]
                .try_into()
                .unwrap_or_else(|_| panic!("test PNG IDAT length is four bytes")),
        ) as usize;
        let idat_crc_offset = idat_offset + 4 + idat_length;
        corrupt_png[idat_crc_offset] ^= 1;
        let mut corrupt_webp = encoded_test_image(ImageFormat::WebP);
        corrupt_webp[12..16].copy_from_slice(b"NOPE");

        for (mime_type, bytes) in [
            ("image/jpeg", corrupt_jpeg.as_slice()),
            ("image/png", corrupt_png.as_slice()),
            ("image/webp", corrupt_webp.as_slice()),
        ] {
            assert_invalid_image_falls_back_to_text_only(mime_type, bytes).await;
        }
    }
    #[tokio::test]
    async fn provider_serializes_zero_one_and_four_images_and_reports_partial_evidence() {
        for count in [0, 1, 4] {
            let (endpoint, task) = serve_once(
                r#"{"success":true,"result":{"model":"clef-flash","answers":{"hard_conflict":{"type":"noul","noul":0.9},"should_show":{"type":"noul","noul":0.6}},"usage":{"input_tokens":42}}}"#.into(), "200 OK",
            ).await;
            let model = CloudflareClassifierModel::with_endpoint(
                config(CloudflareModel::ClefFlash),
                endpoint,
            )
            .unwrap();
            let urls = (0..count)
                .map(|index| Url::parse(&format!("https://example.test/{index}.png")).unwrap())
                .collect::<Vec<_>>();
            let images = (0..count)
                .map(|_| {
                    cloudflare_image_parts(
                        "image/png",
                        &BASE64.encode(encoded_test_image(ImageFormat::Png)),
                    )
                    .unwrap()
                })
                .collect();
            let response = model
                .classify_with_images(request(urls), images, count, None, Duration::ZERO)
                .await
                .unwrap();
            let wire = task.await.unwrap();
            let body: Value = serde_json::from_str(wire.split_once("\r\n\r\n").unwrap().1).unwrap();
            assert_eq!(count, body["images"].as_array().map_or(0, Vec::len));
            assert_eq!(count, response.diagnostics.sent_image_count);
            assert_eq!(count, response.diagnostics.requested_image_count);
            assert_eq!(Some(42), response.usage.input_tokens);
        }
    }

    #[tokio::test]
    async fn provider_batch_bounds_the_complete_request_lifecycle() {
        use std::sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        };
        use tokio::{sync::Barrier, task::JoinSet};

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint =
            Url::parse(&format!("http://{}/test", listener.local_addr().unwrap())).unwrap();
        let active = Arc::new(AtomicUsize::new(0));
        let peak = Arc::new(AtomicUsize::new(0));
        let server_peak = Arc::clone(&peak);
        // Each wave must fill the configured concurrency before it can finish.
        let barrier = Arc::new(Barrier::new(3));
        let server = tokio::spawn(async move {
            let mut workers = JoinSet::new();
            for _ in 0..6 {
                let (mut socket, _) = listener.accept().await.unwrap();
                let active = Arc::clone(&active);
                let peak = Arc::clone(&server_peak);
                let barrier = Arc::clone(&barrier);
                workers.spawn(async move {
                    read_http_request(&mut socket).await;
                    let count = active.fetch_add(1, Ordering::SeqCst) + 1;
                    peak.fetch_max(count, Ordering::SeqCst);
                    barrier.wait().await;
                    let body = r#"{"success":true,"result":{"model":"clef-flash","answers":{"hard_conflict":{"type":"noul","noul":0.1},"should_show":{"type":"noul","noul":0.75}}}}"#;
                    let response = format!(
                        "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                        body.len()
                    );
                    active.fetch_sub(1, Ordering::SeqCst);
                    socket.write_all(response.as_bytes()).await.unwrap();
                });
            }
            while let Some(worker) = workers.join_next().await {
                worker.unwrap();
            }
        });
        let model =
            CloudflareClassifierModel::with_endpoint(config(CloudflareModel::ClefFlash), endpoint)
                .unwrap();
        let results = tokio::time::timeout(
            Duration::from_secs(5),
            model.classify_batch(
                vec![request(Vec::new()); 6],
                ClassificationBatchOptions::new(NonZeroUsize::new(3).unwrap()),
            ),
        )
        .await
        .expect("both bounded request waves complete");
        server.await.unwrap();
        assert_eq!(6, results.len());
        assert!(results.iter().all(Result::is_ok));
        assert_eq!(3, peak.load(Ordering::SeqCst));
    }

    #[test]
    fn request_preparation_deduplicates_urls_and_bytes_and_keeps_partial_failures_explicit() {
        let urls = (0..4)
            .map(|index| Url::parse(&format!("https://example.test/{index}.png")).unwrap())
            .collect::<Vec<_>>();
        let image = cloudflare_image_parts(
            "image/png",
            &BASE64.encode(encoded_test_image(ImageFormat::Png)),
        )
        .unwrap();
        let good = ImagePreparation {
            image: Some(image),
            ..Default::default()
        };
        let failed = ImagePreparation {
            omission_reason: Some(ImageOmissionReason::FetchFailed),
            ..Default::default()
        };
        let request = request(vec![
            urls[0].clone(),
            urls[1].clone(),
            urls[2].clone(),
            urls[2].clone(),
        ]);
        let results = BTreeMap::from([
            (urls[0].to_string(), good.clone()),
            (urls[1].to_string(), failed),
            (urls[2].to_string(), good),
        ]);
        let (images, omission, _) = prepare_request_images(&request, &results);
        assert_eq!(1, images.len());
        assert_eq!(Some(ImageOmissionReason::FetchFailed), omission);
        assert_eq!(
            3,
            unique_image_urls(&[request], CloudflareModel::ClefFlash).len()
        );
    }

    #[test]
    fn aggregate_image_limit_omits_later_images_and_context_overflow_is_explicit() {
        let urls = (0..4)
            .map(|index| Url::parse(&format!("https://example.test/{index}.png")).unwrap())
            .collect::<Vec<_>>();
        let mut results = BTreeMap::new();
        for (index, url) in urls.iter().enumerate() {
            let byte = u8::try_from(index).unwrap();
            results.insert(
                url.to_string(),
                ImagePreparation {
                    image: Some(PreparedCloudflareImage {
                        mime_type: "image/png",
                        base64_data: BASE64.encode(vec![byte; 3 * 1024 * 1024]),
                    }),
                    ..Default::default()
                },
            );
        }
        let (images, reason, _) = prepare_request_images(&request(urls), &results);
        assert_eq!(2, images.len());
        assert_eq!(Some(ImageOmissionReason::AggregateLimit), reason);
        let mut oversized = request(Vec::new());
        oversized.state = serde_json::json!({"criteria": "x".repeat(CloudflareModel::ClefFlash.context_tokens())});
        assert_eq!(
            Err(ClassificationError::PermanentCandidateFailure),
            validate_request(&oversized, CloudflareModel::ClefFlash)
        );
        let bad = BASE64.encode(vec![0; MAX_IMAGE_BYTES + 1]);
        assert_eq!(
            Err(ImageOmissionReason::InvalidOrOversized),
            cloudflare_image_parts("image/png", &bad)
        );
    }
}
