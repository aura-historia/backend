use crate::llm_runtime::{
    CrawlerLlmGovernor, ValidatedGenerationError, generate_validated_with_governor,
};
use crate::scraper::normalization::state::{ProductStateMappingRecord, StateMappingType};
use crate::scraper::normalization::state_mapping_repository::ProductStateMappingRepository;
use large_language_model::{
    GenerationOptions, LargeLanguageModel, LargeLanguageModelError, LlmOperation,
    StructuredGenerationRequest,
};
use product_core::product_state::ProductState;
use regex::Regex;
use schemars::{JsonSchema, schema_for};
use serde::{Deserialize, Serialize};
use std::{sync::Arc, time::Duration};
use time::OffsetDateTime;
use tracing::{debug, warn};

/// Maximum byte length accepted for a raw state string before we reject it
/// (and signal that the CSS selector is extracting wrong content).
///
/// PostgreSQL B-tree indexes cap at ~2704 bytes for `TEXT PRIMARY KEY`, and a
/// legitimate state string is never more than a few words. Any input longer
/// than this constant is almost certainly garbage extracted by a badly-targeted
/// CSS selector.
pub const MAX_STATE_RAW_LEN: usize = 512;

const STATE_MAPPING_SYSTEM_INSTRUCTION: &str = "\
You are a classification assistant for an antiques e-commerce platform.\n\
Your task: map a raw product-state string scraped from a web page to one of \
these normalised states:\n\n\
  LISTED, AVAILABLE, RESERVED, SOLD, REMOVED, UNKNOWN\n\n\
State definitions:\n\
  LISTED    — product is listed but purchase availability is not stated; this is most common for auctions where items are visible before being sold.\n\
  AVAILABLE — product is in stock or can be purchased.\n\
  RESERVED  — product is on hold for a buyer.\n\
  SOLD      — product has been sold or is out of stock.\n\
  REMOVED   — product page has been deleted or is unavailable.\n\
  UNKNOWN   — none of the above clearly applies.\n\n\
Choose a direct state mapping when the raw string is an exact phrase or word. \
Choose a regex mapping only when the raw string is a quantity or template \
pattern that should match many similar inputs. A regex must use valid Rust \
`regex` crate syntax and will be matched against trimmed, lower-cased input. \
Prefer a direct state mapping unless the input clearly contains a variable \
quantity or template placeholder. Return only JSON matching the response schema.";

#[derive(Debug, thiserror::Error)]
pub enum StateMappingServiceError {
    #[error(transparent)]
    LargeLanguageModelError(#[from] LargeLanguageModelError),

    #[error("LLM returned an invalid state mapping response")]
    UnparsableResponse,

    #[error("failed to serialize state mapping response JSON schema")]
    ResponseJsonSchemaSerialization(#[source] serde_json::Error),

    #[error("Database error: {0}")]
    DatabaseError(#[from] sqlx::Error),

    #[error("Database error after state-mapping LLM call: {0}")]
    DatabaseErrorAfterLlm(#[source] sqlx::Error),

    #[error(
        "state text too long ({len} bytes, max {max}): CSS selector is likely extracting wrong content"
    )]
    RawStateTooLong { len: usize, max: usize },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
enum LlmProductState {
    Listed,
    Available,
    Reserved,
    Sold,
    Removed,
    Unknown,
}

impl From<LlmProductState> for ProductState {
    fn from(value: LlmProductState) -> Self {
        match value {
            LlmProductState::Listed => Self::Listed,
            LlmProductState::Available => Self::Available,
            LlmProductState::Reserved => Self::Reserved,
            LlmProductState::Sold => Self::Sold,
            LlmProductState::Removed => Self::Removed,
            LlmProductState::Unknown => Self::Unknown,
        }
    }
}

/// Typed response supplied by structured LLM generation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "mapping_type", rename_all = "snake_case")]
enum LlmMappingResponse {
    State {
        state: LlmProductState,
    },
    Regex {
        pattern: String,
        state: LlmProductState,
    },
}

#[derive(Debug, PartialEq)]
enum ParsedLlmMappingResponse {
    State(ProductState),
    Regex {
        pattern: String,
        state: ProductState,
    },
}

/// Validate the semantic parts that remain outside the JSON schema.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
enum StateMappingResponseValidationError {
    #[error("regex pattern is empty or invalid")]
    InvalidRegex,
}

impl StateMappingResponseValidationError {
    const fn feedback_code(&self) -> &'static str {
        match self {
            Self::InvalidRegex => "invalid_regex",
        }
    }
}

fn validate_llm_response(
    response: LlmMappingResponse,
) -> Result<ParsedLlmMappingResponse, StateMappingResponseValidationError> {
    match response {
        LlmMappingResponse::State { state } => Ok(ParsedLlmMappingResponse::State(state.into())),
        LlmMappingResponse::Regex { pattern, state } => {
            if pattern.trim().is_empty() || Regex::new(&pattern).is_err() {
                return Err(StateMappingResponseValidationError::InvalidRegex);
            }
            Ok(ParsedLlmMappingResponse::Regex {
                pattern,
                state: state.into(),
            })
        }
    }
}

#[cfg(test)]
fn parse_llm_response(
    response: LlmMappingResponse,
) -> Result<ParsedLlmMappingResponse, StateMappingServiceError> {
    validate_llm_response(response).map_err(|_| StateMappingServiceError::UnparsableResponse)
}

fn map_state_mapping_generation_error(
    error: ValidatedGenerationError<StateMappingResponseValidationError>,
) -> StateMappingServiceError {
    match error {
        ValidatedGenerationError::Model(error) => {
            StateMappingServiceError::LargeLanguageModelError(error)
        }
        ValidatedGenerationError::Validation(_) => StateMappingServiceError::UnparsableResponse,
    }
}

fn state_mapping_request(
    normalized_raw_state: String,
) -> Result<StructuredGenerationRequest, StateMappingServiceError> {
    Ok(StructuredGenerationRequest {
        operation: LlmOperation::CrawlerProductStateMapping,
        system_instruction: STATE_MAPPING_SYSTEM_INSTRUCTION.to_owned(),
        prompt: normalized_raw_state,
        image_urls: Vec::new(),
        response_json_schema: serde_json::to_value(schema_for!(LlmMappingResponse))
            .map_err(StateMappingServiceError::ResponseJsonSchemaSerialization)?,
        options: GenerationOptions {
            temperature: 0.0,
            max_output_tokens: 256,
            request_timeout: Duration::from_secs(60),
        },
    })
}

#[async_trait::async_trait]
#[mockall::automock]
pub trait ProductStateMappingService: Send + Sync {
    /// Ask the LLM to classify a raw state string.
    ///
    /// The LLM may respond with either a direct state word or a regex pattern.
    /// The returned record is **not** persisted — call [`save_state_mapping`] or
    /// use [`get_state_mapping`] to persist automatically.
    async fn create_state_mapping(
        &self,
        raw: &str,
    ) -> Result<ProductStateMappingRecord, StateMappingServiceError>;

    /// Look up a mapping by exact key (returns `None` on miss).
    async fn find_state_mapping(
        &self,
        raw: &str,
    ) -> Result<Option<ProductStateMappingRecord>, StateMappingServiceError>;

    /// Persist a mapping: insert if new, update if already present.
    async fn save_state_mapping(
        &self,
        raw: &str,
        normalized: ProductState,
        mapping_type: StateMappingType,
    ) -> Result<ProductStateMappingRecord, StateMappingServiceError>;

    /// High-level entry-point.
    ///
    /// Resolution order:
    /// 1. Exact DB key lookup on the trimmed, lowercased input.
    /// 2. Scan all regex mappings from the DB; return on first match.
    /// 3. Ask the LLM; persist and return the result.
    ///
    /// Returns `(record, llm_called)` where `llm_called` is `true` only when
    /// the LLM fallback (step 3) was reached. Callers use this flag to charge
    /// the resolved mapping against the per-shop LLM budget.
    async fn get_state_mapping(
        &self,
        raw: &str,
    ) -> Result<(ProductStateMappingRecord, bool), StateMappingServiceError>;
}

pub struct ProductStateMappingServiceImpl<Llm> {
    llm: Llm,
    governor: Option<Arc<CrawlerLlmGovernor>>,
    repository: Box<dyn ProductStateMappingRepository + Send + Sync>,
}

impl<Llm> ProductStateMappingServiceImpl<Llm>
where
    Llm: LargeLanguageModel,
{
    pub fn new(
        llm: Llm,
        repository: Box<dyn ProductStateMappingRepository + Send + Sync>,
        governor: Option<Arc<CrawlerLlmGovernor>>,
    ) -> Self {
        Self {
            llm,
            governor,
            repository,
        }
    }
}

#[async_trait::async_trait]
impl<Llm> ProductStateMappingService for ProductStateMappingServiceImpl<Llm>
where
    Llm: LargeLanguageModel,
{
    #[tracing::instrument(
        name = "create_product_state_mapping",
        skip_all,
        fields(raw_length = raw.len())
    )]
    async fn create_state_mapping(
        &self,
        raw: &str,
    ) -> Result<ProductStateMappingRecord, StateMappingServiceError> {
        let key = raw.trim().to_lowercase();
        let parsed = generate_validated_with_governor::<
            _,
            LlmMappingResponse,
            ParsedLlmMappingResponse,
            StateMappingResponseValidationError,
            _,
            _,
        >(
            &self.llm,
            self.governor.as_ref(),
            state_mapping_request(key.clone())?,
            3,
            validate_llm_response,
            StateMappingResponseValidationError::feedback_code,
        )
        .await
        .map_err(map_state_mapping_generation_error)?;
        let mapping_type = match &parsed {
            ParsedLlmMappingResponse::State(_) => "value",
            ParsedLlmMappingResponse::Regex { .. } => "regex",
        };
        debug!(
            mapping_type,
            "LLM state mapping response parsed successfully"
        );

        let now = OffsetDateTime::now_utc();
        Ok(match parsed {
            ParsedLlmMappingResponse::State(normalized) => ProductStateMappingRecord {
                raw: key,
                normalized,
                mapping_type: StateMappingType::Value,
                created: now,
                updated: now,
            },
            ParsedLlmMappingResponse::Regex { pattern, state } => ProductStateMappingRecord {
                raw: pattern,
                normalized: state,
                mapping_type: StateMappingType::Regex,
                created: now,
                updated: now,
            },
        })
    }

    async fn find_state_mapping(
        &self,
        raw: &str,
    ) -> Result<Option<ProductStateMappingRecord>, StateMappingServiceError> {
        let key = raw.trim().to_lowercase();
        self.repository
            .find_mapping(&key)
            .await
            .map_err(StateMappingServiceError::DatabaseError)
    }

    #[tracing::instrument(
        name = "save_product_state_mapping",
        skip_all,
        fields(raw_length = raw.len(), mapping_type = ?mapping_type)
    )]
    async fn save_state_mapping(
        &self,
        raw: &str,
        normalized: ProductState,
        mapping_type: StateMappingType,
    ) -> Result<ProductStateMappingRecord, StateMappingServiceError> {
        let key = raw.trim().to_lowercase();
        let existing = self.repository.find_mapping(&key).await?;

        match existing {
            Some(_) => {
                debug!("Updating existing state mapping");
                self.repository
                    .update_mapping(&key, &normalized, &mapping_type)
                    .await
                    .map_err(StateMappingServiceError::DatabaseError)
            }
            None => {
                debug!("Inserting new state mapping");
                let now = OffsetDateTime::now_utc();
                let record = ProductStateMappingRecord {
                    raw: key,
                    normalized,
                    mapping_type,
                    created: now,
                    updated: now,
                };
                self.repository
                    .insert_mapping(&record)
                    .await
                    .map_err(StateMappingServiceError::DatabaseError)
            }
        }
    }

    #[tracing::instrument(
        name = "get_product_state_mapping",
        skip_all,
        fields(raw_length = raw.len())
    )]
    async fn get_state_mapping(
        &self,
        raw: &str,
    ) -> Result<(ProductStateMappingRecord, bool), StateMappingServiceError> {
        let key = raw.trim().to_lowercase();

        if key.len() > MAX_STATE_RAW_LEN {
            warn!(
                len = key.len(),
                max = MAX_STATE_RAW_LEN,
                "Rejecting state text that exceeds the mapping limit"
            );
            return Err(StateMappingServiceError::RawStateTooLong {
                len: key.len(),
                max: MAX_STATE_RAW_LEN,
            });
        }

        if let Some(existing) = self.repository.find_mapping(&key).await? {
            debug!("Found exact state mapping in database");
            return Ok((existing, false));
        }

        let regex_mappings = self.repository.find_all_regex_mappings().await?;
        for mapping in &regex_mappings {
            match Regex::new(&mapping.raw) {
                Ok(regex) if regex.is_match(&key) => {
                    debug!("Matched state with stored regex mapping");
                    return Ok((mapping.clone(), false));
                }
                Ok(_) => {}
                Err(_) => {
                    warn!(
                        error_kind = "invalid_regex",
                        "Skipping invalid stored product state regex mapping"
                    );
                }
            }
        }

        debug!("No stored state mapping found; requesting LLM fallback");
        let record = self.create_state_mapping(&key).await?;
        let (persist_key, normalized, mapping_type) =
            (record.raw.clone(), record.normalized, record.mapping_type);
        let persisted = self
            .save_state_mapping(&persist_key, normalized, mapping_type)
            .await
            .map_err(|error| match error {
                StateMappingServiceError::DatabaseError(source) => {
                    StateMappingServiceError::DatabaseErrorAfterLlm(source)
                }
                other => other,
            })?;
        Ok((persisted, true))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scraper::normalization::state_mapping_repository::MockProductStateMappingRepository;
    use serde::de::DeserializeOwned;
    use std::collections::VecDeque;
    use std::sync::{Arc, Mutex};

    struct NoCallLlm;

    #[async_trait::async_trait]
    impl LargeLanguageModel for NoCallLlm {
        async fn generate<Output>(
            &self,
            _: StructuredGenerationRequest,
        ) -> Result<Output, LargeLanguageModelError>
        where
            Output: DeserializeOwned + Send,
        {
            panic!("LLM should not be called in this test")
        }
    }

    #[derive(Clone)]
    struct CapturingLlm {
        response: serde_json::Value,
        request: Arc<Mutex<Option<StructuredGenerationRequest>>>,
    }

    impl CapturingLlm {
        fn responding(response: LlmMappingResponse) -> Self {
            let response = serde_json::to_value(response)
                .unwrap_or_else(|error| panic!("test response must serialize: {error}"));
            Self {
                response,
                request: Arc::new(Mutex::new(None)),
            }
        }

        fn request(&self) -> Result<Option<StructuredGenerationRequest>, &'static str> {
            self.request
                .lock()
                .map(|request| request.clone())
                .map_err(|_| "captured LLM request mutex was poisoned")
        }
    }

    #[async_trait::async_trait]
    impl LargeLanguageModel for CapturingLlm {
        async fn generate<Output>(
            &self,
            request: StructuredGenerationRequest,
        ) -> Result<Output, LargeLanguageModelError>
        where
            Output: DeserializeOwned + Send,
        {
            let mut captured =
                self.request
                    .lock()
                    .map_err(|_| LargeLanguageModelError::InvalidResponse {
                        source: Box::new(std::io::Error::other(
                            "captured LLM request mutex was poisoned",
                        )),
                    })?;
            *captured = Some(request);
            serde_json::from_value(self.response.clone()).map_err(|source| {
                LargeLanguageModelError::InvalidResponse {
                    source: Box::new(source),
                }
            })
        }
    }

    struct SequenceMappingLlm {
        responses: Mutex<VecDeque<serde_json::Value>>,
        requests: Arc<Mutex<Vec<StructuredGenerationRequest>>>,
    }

    #[async_trait::async_trait]
    impl LargeLanguageModel for SequenceMappingLlm {
        async fn generate<Output>(
            &self,
            request: StructuredGenerationRequest,
        ) -> Result<Output, LargeLanguageModelError>
        where
            Output: DeserializeOwned + Send,
        {
            self.requests
                .lock()
                .map_err(|error| LargeLanguageModelError::InvalidResponse {
                    source: Box::new(std::io::Error::other(error.to_string())),
                })?
                .push(request);
            let response = self
                .responses
                .lock()
                .map_err(|error| LargeLanguageModelError::InvalidResponse {
                    source: Box::new(std::io::Error::other(error.to_string())),
                })?
                .pop_front()
                .ok_or_else(|| LargeLanguageModelError::Permanent {
                    source: Box::new(std::io::Error::other("mapping response sequence exhausted")),
                })?;
            serde_json::from_value(response).map_err(|source| {
                LargeLanguageModelError::InvalidResponse {
                    source: Box::new(source),
                }
            })
        }
    }

    struct ErrorLlm;

    #[async_trait::async_trait]
    impl LargeLanguageModel for ErrorLlm {
        async fn generate<Output>(
            &self,
            _: StructuredGenerationRequest,
        ) -> Result<Output, LargeLanguageModelError>
        where
            Output: DeserializeOwned + Send,
        {
            Err(LargeLanguageModelError::Permanent {
                source: Box::new(std::io::Error::other("simulated")),
            })
        }
    }

    fn value_record(raw: &str, state: ProductState) -> ProductStateMappingRecord {
        let now = OffsetDateTime::now_utc();
        ProductStateMappingRecord {
            raw: raw.to_owned(),
            normalized: state,
            mapping_type: StateMappingType::Value,
            created: now,
            updated: now,
        }
    }

    fn regex_record(pattern: &str, state: ProductState) -> ProductStateMappingRecord {
        let now = OffsetDateTime::now_utc();
        ProductStateMappingRecord {
            raw: pattern.to_owned(),
            normalized: state,
            mapping_type: StateMappingType::Regex,
            created: now,
            updated: now,
        }
    }

    fn service<Llm>(
        llm: Llm,
        repository: MockProductStateMappingRepository,
    ) -> ProductStateMappingServiceImpl<Llm>
    where
        Llm: LargeLanguageModel,
    {
        ProductStateMappingServiceImpl::new(llm, Box::new(repository), None)
    }

    fn record_matches(
        record: &ProductStateMappingRecord,
        raw: &str,
        normalized: ProductState,
        mapping_type: StateMappingType,
    ) -> bool {
        record.raw == raw && record.normalized == normalized && record.mapping_type == mapping_type
    }

    #[test]
    fn should_parse_direct_structured_response() {
        let parsed = parse_llm_response(LlmMappingResponse::State {
            state: LlmProductState::Available,
        });

        assert!(matches!(
            parsed,
            Ok(ParsedLlmMappingResponse::State(ProductState::Available))
        ));
    }

    #[test]
    fn should_parse_valid_regex_structured_response() {
        let parsed = parse_llm_response(LlmMappingResponse::Regex {
            pattern: r"[1-9][0-9]*\s+in\s+stock\b".to_owned(),
            state: LlmProductState::Available,
        });

        assert!(matches!(
            parsed,
            Ok(ParsedLlmMappingResponse::Regex { pattern, state })
                if pattern == r"[1-9][0-9]*\s+in\s+stock\b"
                    && state == ProductState::Available
        ));
    }

    #[tokio::test]
    async fn should_correct_invalid_regex_before_returning_state_mapping() {
        let requests = Arc::new(Mutex::new(Vec::new()));
        let llm = SequenceMappingLlm {
            responses: Mutex::new(VecDeque::from([
                serde_json::json!({
                    "mapping_type": "regex",
                    "pattern": "[",
                    "state": "AVAILABLE"
                }),
                serde_json::json!({
                    "mapping_type": "state",
                    "state": "SOLD"
                }),
            ])),
            requests: Arc::clone(&requests),
        };
        let service = service(llm, MockProductStateMappingRepository::new());

        let result = service.create_state_mapping("sold").await;
        assert!(matches!(
            result,
            Ok(ProductStateMappingRecord {
                normalized: ProductState::Sold,
                mapping_type: StateMappingType::Value,
                ..
            })
        ));
        let requests = requests.lock().unwrap_or_else(|error| error.into_inner());
        assert_eq!(requests.len(), 2);
        assert!(requests[1].prompt.contains("invalid_regex"));
    }

    #[test]
    fn should_reject_blank_or_invalid_regex_structured_response() {
        let blank = parse_llm_response(LlmMappingResponse::Regex {
            pattern: " ".to_owned(),
            state: LlmProductState::Available,
        });
        let invalid = parse_llm_response(LlmMappingResponse::Regex {
            pattern: "[unclosed".to_owned(),
            state: LlmProductState::Available,
        });

        assert!(matches!(
            blank,
            Err(StateMappingServiceError::UnparsableResponse)
        ));
        assert!(matches!(
            invalid,
            Err(StateMappingServiceError::UnparsableResponse)
        ));
    }

    #[tokio::test]
    async fn should_build_structured_state_mapping_request() {
        let llm = CapturingLlm::responding(LlmMappingResponse::State {
            state: LlmProductState::Sold,
        });
        let probe = llm.clone();
        let service = service(llm, MockProductStateMappingRepository::new());

        let result = service.create_state_mapping("  SOLD OUT ").await;
        assert!(result.is_ok());

        let request = match probe.request() {
            Ok(Some(request)) => request,
            Ok(None) => panic!("LLM request should be captured"),
            Err(error) => panic!("failed to read captured LLM request: {error}"),
        };
        assert_eq!(request.operation, LlmOperation::CrawlerProductStateMapping);
        assert_eq!(request.system_instruction, STATE_MAPPING_SYSTEM_INSTRUCTION);
        assert_eq!(request.prompt, "sold out");
        assert!(request.image_urls.is_empty());
        assert_eq!(request.options.temperature, 0.0);
        assert_eq!(request.options.max_output_tokens, 256);
        assert!(request.response_json_schema.is_object());
    }

    #[tokio::test]
    async fn should_create_value_mapping_from_structured_llm_response() {
        let llm = CapturingLlm::responding(LlmMappingResponse::State {
            state: LlmProductState::Reserved,
        });
        let service = service(llm, MockProductStateMappingRepository::new());

        let result = service.create_state_mapping(" On hold ").await;

        assert!(matches!(
            result,
            Ok(ProductStateMappingRecord {
                raw,
                normalized: ProductState::Reserved,
                mapping_type: StateMappingType::Value,
                ..
            }) if raw == "on hold"
        ));
    }

    #[tokio::test]
    async fn should_create_regex_mapping_from_structured_llm_response() {
        let pattern = r"\b(only\s+)?[1-9][0-9]*\s+left\b";
        let llm = CapturingLlm::responding(LlmMappingResponse::Regex {
            pattern: pattern.to_owned(),
            state: LlmProductState::Available,
        });
        let service = service(llm, MockProductStateMappingRepository::new());

        let result = service.create_state_mapping("only 3 left").await;

        assert!(matches!(
            result,
            Ok(ProductStateMappingRecord {
                raw,
                normalized: ProductState::Available,
                mapping_type: StateMappingType::Regex,
                ..
            }) if raw == pattern
        ));
    }

    #[tokio::test]
    async fn should_map_large_language_model_error_when_generation_fails() {
        let service = service(ErrorLlm, MockProductStateMappingRepository::new());

        let error = service.create_state_mapping("sold out").await.err();

        assert!(matches!(
            error,
            Some(StateMappingServiceError::LargeLanguageModelError(
                LargeLanguageModelError::Permanent { .. }
            ))
        ));
    }

    #[tokio::test]
    async fn should_find_normalized_exact_mapping_without_calling_llm() {
        let expected = value_record("sold", ProductState::Sold);
        let expected_clone = expected.clone();
        let mut repository = MockProductStateMappingRepository::new();
        repository
            .expect_find_mapping()
            .withf(|raw| raw == "sold")
            .return_once(move |_| Box::pin(async move { Ok(Some(expected_clone)) }));
        let service = service(NoCallLlm, repository);

        let result = service.find_state_mapping("  SOLD ").await;

        assert!(matches!(
            result,
            Ok(Some(record))
                if record_matches(&record, "sold", ProductState::Sold, StateMappingType::Value)
        ));
    }

    #[tokio::test]
    async fn should_propagate_database_error_when_exact_lookup_fails() {
        let mut repository = MockProductStateMappingRepository::new();
        repository
            .expect_find_mapping()
            .return_once(|_| Box::pin(async { Err(sqlx::Error::RowNotFound) }));
        let service = service(NoCallLlm, repository);

        let result = service.find_state_mapping("sold").await;

        assert!(matches!(
            result,
            Err(StateMappingServiceError::DatabaseError(_))
        ));
    }

    #[tokio::test]
    async fn should_insert_mapping_when_no_existing_mapping() {
        let expected = value_record("nuevo", ProductState::Available);
        let expected_clone = expected.clone();
        let mut repository = MockProductStateMappingRepository::new();
        repository
            .expect_find_mapping()
            .withf(|raw| raw == "nuevo")
            .return_once(|_| Box::pin(async { Ok(None) }));
        repository
            .expect_insert_mapping()
            .withf(|record| {
                record.raw == "nuevo"
                    && record.normalized == ProductState::Available
                    && record.mapping_type == StateMappingType::Value
            })
            .return_once(move |_| Box::pin(async move { Ok(expected_clone) }));
        let service = service(NoCallLlm, repository);

        let result = service
            .save_state_mapping(" NUEVO ", ProductState::Available, StateMappingType::Value)
            .await;

        assert!(matches!(
            result,
            Ok(record)
                if record_matches(&record, "nuevo", ProductState::Available, StateMappingType::Value)
        ));
    }

    #[tokio::test]
    async fn should_update_mapping_when_existing_mapping_is_found() {
        let existing = value_record("sold out", ProductState::Sold);
        let updated = value_record("sold out", ProductState::Removed);
        let updated_clone = updated.clone();
        let mut repository = MockProductStateMappingRepository::new();
        repository
            .expect_find_mapping()
            .withf(|raw| raw == "sold out")
            .return_once(move |_| Box::pin(async move { Ok(Some(existing)) }));
        repository
            .expect_update_mapping()
            .withf(|raw, state, mapping_type| {
                raw == "sold out"
                    && *state == ProductState::Removed
                    && *mapping_type == StateMappingType::Value
            })
            .return_once(move |_, _, _| Box::pin(async move { Ok(updated_clone) }));
        let service = service(NoCallLlm, repository);

        let result = service
            .save_state_mapping("sold out", ProductState::Removed, StateMappingType::Value)
            .await;

        assert!(matches!(
            result,
            Ok(record)
                if record_matches(&record, "sold out", ProductState::Removed, StateMappingType::Value)
        ));
    }

    #[tokio::test]
    async fn should_return_exact_mapping_before_regex_or_llm() {
        let expected = value_record("sold", ProductState::Sold);
        let expected_clone = expected.clone();
        let mut repository = MockProductStateMappingRepository::new();
        repository
            .expect_find_mapping()
            .withf(|raw| raw == "sold")
            .return_once(move |_| Box::pin(async move { Ok(Some(expected_clone)) }));
        let service = service(NoCallLlm, repository);

        let result = service.get_state_mapping("sold").await;

        assert!(matches!(
            result,
            Ok((record, false))
                if record_matches(&record, "sold", ProductState::Sold, StateMappingType::Value)
        ));
    }

    #[tokio::test]
    async fn should_return_first_matching_stored_regex_before_llm() {
        let matching = regex_record(r"[1-9][0-9]*\s+in\s+stock\b", ProductState::Available);
        let matching_clone = matching.clone();
        let mut repository = MockProductStateMappingRepository::new();
        repository
            .expect_find_mapping()
            .withf(|raw| raw == "5 in stock")
            .return_once(|_| Box::pin(async { Ok(None) }));
        repository
            .expect_find_all_regex_mappings()
            .return_once(move || Box::pin(async move { Ok(vec![matching_clone]) }));
        let service = service(NoCallLlm, repository);

        let result = service.get_state_mapping("5 in stock").await;

        assert!(matches!(
            result,
            Ok((record, false))
                if record_matches(
                    &record,
                    r"[1-9][0-9]*\s+in\s+stock\b",
                    ProductState::Available,
                    StateMappingType::Regex,
                )
        ));
    }

    #[tokio::test]
    async fn should_skip_invalid_stored_regex_and_use_next_match() {
        let matching = regex_record(r"\bsold\s+out\b", ProductState::Sold);
        let matching_clone = matching.clone();
        let mut repository = MockProductStateMappingRepository::new();
        repository
            .expect_find_mapping()
            .return_once(|_| Box::pin(async { Ok(None) }));
        repository
            .expect_find_all_regex_mappings()
            .return_once(move || {
                Box::pin(async move {
                    Ok(vec![
                        regex_record("[unclosed", ProductState::Unknown),
                        matching_clone,
                    ])
                })
            });
        let service = service(NoCallLlm, repository);

        let result = service.get_state_mapping("sold out").await;

        assert!(matches!(
            result,
            Ok((record, false))
                if record_matches(
                    &record,
                    r"\bsold\s+out\b",
                    ProductState::Sold,
                    StateMappingType::Regex,
                )
        ));
    }

    #[tokio::test]
    async fn should_generate_and_persist_value_mapping_after_lookup_misses() {
        let saved = value_record("brand new phrase", ProductState::Listed);
        let saved_clone = saved.clone();
        let mut repository = MockProductStateMappingRepository::new();
        repository
            .expect_find_mapping()
            .returning(|_| Box::pin(async { Ok(None) }));
        repository
            .expect_find_all_regex_mappings()
            .return_once(|| Box::pin(async { Ok(Vec::new()) }));
        repository
            .expect_insert_mapping()
            .withf(|record| {
                record.raw == "brand new phrase"
                    && record.normalized == ProductState::Listed
                    && record.mapping_type == StateMappingType::Value
            })
            .return_once(move |_| Box::pin(async move { Ok(saved_clone) }));
        let llm = CapturingLlm::responding(LlmMappingResponse::State {
            state: LlmProductState::Listed,
        });
        let service = service(llm, repository);

        let result = service.get_state_mapping("brand new phrase").await;

        assert!(matches!(
            result,
            Ok((record, true))
                if record_matches(
                    &record,
                    "brand new phrase",
                    ProductState::Listed,
                    StateMappingType::Value,
                )
        ));
    }

    #[tokio::test]
    async fn should_generate_and_persist_regex_mapping_after_lookup_misses() {
        let pattern = r"[1-9][0-9]*\s+verfügbar\b";
        let saved = regex_record(pattern, ProductState::Available);
        let saved_clone = saved.clone();
        let mut repository = MockProductStateMappingRepository::new();
        repository
            .expect_find_mapping()
            .returning(|_| Box::pin(async { Ok(None) }));
        repository
            .expect_find_all_regex_mappings()
            .return_once(|| Box::pin(async { Ok(Vec::new()) }));
        repository
            .expect_insert_mapping()
            .withf(move |record| {
                record.raw == pattern
                    && record.normalized == ProductState::Available
                    && record.mapping_type == StateMappingType::Regex
            })
            .return_once(move |_| Box::pin(async move { Ok(saved_clone) }));
        let llm = CapturingLlm::responding(LlmMappingResponse::Regex {
            pattern: pattern.to_owned(),
            state: LlmProductState::Available,
        });
        let service = service(llm, repository);

        let result = service.get_state_mapping("7 verfügbar").await;

        assert!(matches!(
            result,
            Ok((record, true))
                if record_matches(
                    &record,
                    r"[1-9][0-9]*\s+verfügbar\b",
                    ProductState::Available,
                    StateMappingType::Regex,
                )
        ));
    }

    #[tokio::test]
    async fn should_reject_too_long_state_without_repository_or_llm_access() {
        let raw = "a".repeat(MAX_STATE_RAW_LEN + 1);
        let service = service(NoCallLlm, MockProductStateMappingRepository::new());

        let result = service.get_state_mapping(&raw).await;

        assert!(matches!(
            result,
            Err(StateMappingServiceError::RawStateTooLong { len, max })
                if len == MAX_STATE_RAW_LEN + 1 && max == MAX_STATE_RAW_LEN
        ));
    }
}
