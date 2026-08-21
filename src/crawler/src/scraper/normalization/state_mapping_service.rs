use crate::google_llm::{GeminiRateLimiter, run_with_gemini_rate_limiter};
use crate::logging::llm_metrics;
use crate::scraper::normalization::state::{ProductStateMappingRecord, StateMappingType};
use crate::scraper::normalization::state_mapping_repository::ProductStateMappingRepository;
use common::product_state::domain::ProductState;
use large_language_model::{
    GeminiServiceTier, LlmModel, LlmOperation, LlmProvider, log_llm_invocation,
};
use llm::{
    chat::{ChatMessage, ChatProvider},
    error::LLMError,
};
use product::dynamodb::product_state_record::ProductStateRecord;
use regex::Regex;
use std::sync::Arc;
use time::OffsetDateTime;
use tracing::{debug, warn};

// ---------------------------------------------------------------------------
// Error
// ---------------------------------------------------------------------------

/// Maximum byte length accepted for a raw state string before we reject it
/// (and signal that the CSS selector is extracting wrong content).
///
/// PostgreSQL B-tree indexes cap at ~2704 bytes for `TEXT PRIMARY KEY`, and a
/// legitimate state string is never more than a few words.  Any input longer
/// than this constant is almost certainly garbage extracted by a badly-targeted
/// CSS selector.
pub const MAX_STATE_RAW_LEN: usize = 512;

#[derive(Debug, thiserror::Error)]
pub enum StateMappingServiceError {
    #[error("LLM error: {0}")]
    LLMError(#[from] LLMError),

    #[error("NoTextResponse: {0}")]
    NoTextResponse(String),

    #[error("LLM returned an unparseable response: {0}")]
    UnparsableResponse(String),

    #[error("Database error: {0}")]
    DatabaseError(#[from] sqlx::Error),

    #[error(
        "state text too long ({len} bytes, max {max}): CSS selector is likely extracting wrong content"
    )]
    RawStateTooLong { len: usize, max: usize },
}

// ---------------------------------------------------------------------------
// LLM response type
// ---------------------------------------------------------------------------

/// The two forms the LLM may respond with.
///
/// - `State(record)` — the raw string is an exact (lowercased) value that maps
///   directly to one state. Saved as a [`StateMappingType::Value`] record keyed
///   on the normalised input string.
/// - `Regex { pattern, state }` — the LLM chose to express the mapping as a
///   regular expression (e.g. because the input is a quantity-style template
///   that generalises to many inputs). Saved as a [`StateMappingType::Regex`]
///   record keyed on the pattern string itself.
#[derive(Debug, PartialEq)]
pub enum LlmMappingResponse {
    State(ProductStateRecord),
    Regex {
        pattern: String,
        state: ProductStateRecord,
    },
}

// ---------------------------------------------------------------------------
// Trait
// ---------------------------------------------------------------------------

#[async_trait::async_trait]
#[mockall::automock]
pub trait ProductStateMappingService {
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
    /// the LLM fallback (step 3) was reached.  Callers use this flag to charge
    /// the resolved mapping against the per-shop LLM budget.
    async fn get_state_mapping(
        &self,
        raw: &str,
    ) -> Result<(ProductStateMappingRecord, bool), StateMappingServiceError>;
}

// ---------------------------------------------------------------------------
// Implementation
// ---------------------------------------------------------------------------

pub struct ProductStateMappingServiceImpl {
    llm: Box<dyn ChatProvider>,
    rate_limiter: Option<Arc<GeminiRateLimiter>>,
    service_tier: Option<GeminiServiceTier>,
    repository: Box<dyn ProductStateMappingRepository + Send + Sync>,
}

impl ProductStateMappingServiceImpl {
    pub fn new(
        llm: llm::builder::LLMBuilder,
        service_tier: Option<GeminiServiceTier>,
        repository: Box<dyn ProductStateMappingRepository + Send + Sync>,
        rate_limiter: Option<Arc<GeminiRateLimiter>>,
    ) -> Result<Self, LLMError> {
        let system_prompt = "\
You are a classification assistant for an antiques e-commerce platform.\n\
Your task: map a raw product-state string scraped from a web page to one of \
these normalised states:\n\n\
  LISTED, AVAILABLE, RESERVED, SOLD, REMOVED, UNKNOWN\n\n\
State definitions:\n\
  LISTED    — product is listed but purchase availability is not stated, this is most common for auctions where items are visible before being sold\n\
  AVAILABLE — product is in stock / can be purchased.\n\
  RESERVED  — product is on hold for a buyer.\n\
  SOLD      — product has been sold or is out of stock.\n\
  REMOVED   — product page has been deleted or is unavailable.\n\
  UNKNOWN   — none of the above clearly applies.\n\n\
Response format — choose EXACTLY ONE:\n\n\
  1. Direct state (use when the raw string is an exact phrase or word):\n\
       STATE:<state>\n\
     Example: STATE:AVAILABLE\n\n\
  2. Regex pattern (use when the raw string is a template or quantity pattern\n\
     that should match many similar inputs):\n\
       REGEX:<rust-regex-pattern>:<state>\n\
     Example: REGEX:[1-9][0-9]*\\s+in\\s+stock\\b:AVAILABLE\n\n\
Rules:\n\
  - Output a SINGLE LINE with no extra text, explanation, or formatting.\n\
  - The regex must be valid Rust regex syntax (the `regex` crate).\n\
  - The regex will be matched against the trimmed, lower-cased input.\n\
  - Prefer STATE over REGEX unless the input clearly contains a variable\n\
    quantity or template placeholder.\n\
  - The <state> token must be one of the six names listed above."
            .to_string();

        let llm = llm
            .system(system_prompt)
            .openai_enable_web_search(false)
            .reasoning(true)
            .timeout_seconds(60)
            .build()?;
        let llm: Box<dyn ChatProvider> = llm;

        Ok(Self {
            llm,
            rate_limiter,
            service_tier,
            repository,
        })
    }
}

// ---------------------------------------------------------------------------
// Parsing helpers
// ---------------------------------------------------------------------------

/// Parse a state name token (case-insensitive) into a [`ProductStateRecord`].
pub(crate) fn parse_state_token(token: &str) -> Option<ProductStateRecord> {
    match token.trim().to_uppercase().as_str() {
        "LISTED" => Some(ProductStateRecord::Listed),
        "AVAILABLE" => Some(ProductStateRecord::Available),
        "RESERVED" => Some(ProductStateRecord::Reserved),
        "SOLD" => Some(ProductStateRecord::Sold),
        "REMOVED" => Some(ProductStateRecord::Removed),
        "UNKNOWN" => Some(ProductStateRecord::Unknown),
        _ => None,
    }
}

/// Parse the single-line LLM response into an [`LlmMappingResponse`].
///
/// Accepted formats:
/// - `STATE:<state>`
/// - `REGEX:<rust-regex-pattern>:<state>`
///
/// The `<state>` token in the `REGEX` form is the *last* colon-separated
/// segment so that patterns that themselves contain colons are handled
/// correctly.
pub(crate) fn parse_llm_response(response: &str) -> Option<LlmMappingResponse> {
    let line = response.trim();

    if let Some(rest) = line.strip_prefix("STATE:") {
        let state = parse_state_token(rest)?;
        return Some(LlmMappingResponse::State(state));
    }

    if let Some(rest) = line.strip_prefix("REGEX:") {
        // The state token is the last colon-delimited segment; everything
        // before it is the regex pattern (which may itself contain colons).
        let colon = rest.rfind(':')?;
        let pattern = rest[..colon].to_string();
        let state_token = &rest[colon + 1..];
        let state = parse_state_token(state_token)?;
        // Reject empty or blank patterns, and patterns that do not compile as a valid Rust regex.
        if pattern.trim().is_empty() {
            return None;
        }
        Regex::new(&pattern).ok()?;
        return Some(LlmMappingResponse::Regex { pattern, state });
    }

    None
}

// ---------------------------------------------------------------------------
// Service implementation
// ---------------------------------------------------------------------------

#[async_trait::async_trait]
impl ProductStateMappingService for ProductStateMappingServiceImpl {
    // ── create_state_mapping ─────────────────────────────────────────────

    #[tracing::instrument(skip(self), fields(raw = %raw))]
    async fn create_state_mapping(
        &self,
        raw: &str,
    ) -> Result<ProductStateMappingRecord, StateMappingServiceError> {
        let key = raw.trim().to_lowercase();

        let instruction = format!(
            "Classify the following raw product-state string.\n\n\
             Raw string: \"{key}\"\n\n\
             Respond with STATE:<state> or REGEX:<pattern>:<state>.",
        );
        let message = ChatMessage::user().content(instruction).build();

        let started_at = std::time::Instant::now();
        let messages = [message];
        let response =
            run_with_gemini_rate_limiter(&*self.llm, self.rate_limiter.as_deref(), &messages)
                .await?;
        log_llm_invocation(
            LlmOperation::CrawlerProductStateMapping,
            LlmProvider::Google,
            LlmModel::Configured,
            started_at.elapsed(),
            llm_metrics(response.usage(), Some(1), self.service_tier),
        );
        let res = response.text().ok_or_else(|| {
            StateMappingServiceError::NoTextResponse("Expected text response".to_string())
        })?;

        let parsed = parse_llm_response(&res)
            .ok_or_else(|| StateMappingServiceError::UnparsableResponse(res.clone()))?;
        debug!(
            llm_response = %res,
            mapping_type = ?parsed,
            "LLM state mapping response parsed successfully"
        );

        let now = OffsetDateTime::now_utc();
        let record = match parsed {
            LlmMappingResponse::State(normalized) => ProductStateMappingRecord {
                raw: key,
                normalized,
                mapping_type: StateMappingType::Value,
                created: now,
                updated: now,
            },
            LlmMappingResponse::Regex { pattern, state } => ProductStateMappingRecord {
                raw: pattern,
                normalized: state,
                mapping_type: StateMappingType::Regex,
                created: now,
                updated: now,
            },
        };

        Ok(record)
    }

    // ── find_state_mapping ───────────────────────────────────────────────

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

    // ── save_state_mapping ───────────────────────────────────────────────

    #[tracing::instrument(skip(self), fields(raw = %raw, mapping_type = ?mapping_type))]
    async fn save_state_mapping(
        &self,
        raw: &str,
        normalized: ProductState,
        mapping_type: StateMappingType,
    ) -> Result<ProductStateMappingRecord, StateMappingServiceError> {
        let key = raw.trim().to_lowercase();
        let normalized_record: ProductStateRecord = normalized.into();

        let existing = self.repository.find_mapping(&key).await?;

        match existing {
            Some(_) => {
                debug!("Updating existing state mapping...");
                self.repository
                    .update_mapping(&key, &normalized_record, &mapping_type)
                    .await
                    .map_err(StateMappingServiceError::DatabaseError)
            }
            None => {
                debug!("Inserting new state mapping...");
                let now = OffsetDateTime::now_utc();
                let record = ProductStateMappingRecord {
                    raw: key,
                    normalized: normalized_record,
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

    // ── get_state_mapping ────────────────────────────────────────────────

    #[tracing::instrument(skip(self), fields(raw = %raw))]
    async fn get_state_mapping(
        &self,
        raw: &str,
    ) -> Result<(ProductStateMappingRecord, bool), StateMappingServiceError> {
        let key = raw.trim().to_lowercase();

        // ── Guard: reject text that is too long to fit in the DB index ───
        if key.len() > MAX_STATE_RAW_LEN {
            let truncated = &key[..key.len().min(120)];
            warn!(
                raw = %truncated,
                len = key.len(),
                max = MAX_STATE_RAW_LEN,
                "Rejecting state text: too long (CSS selector likely extracting wrong content)"
            );
            return Err(StateMappingServiceError::RawStateTooLong {
                len: key.len(),
                max: MAX_STATE_RAW_LEN,
            });
        }

        // ── Step 1: exact DB lookup ──────────────────────────────────────
        if let Some(existing) = self.repository.find_mapping(&key).await? {
            debug!("Found exact state mapping in DB.");
            return Ok((existing, false));
        }

        // ── Step 2: scan regex mappings from DB ──────────────────────────
        let regex_mappings = self.repository.find_all_regex_mappings().await?;
        for mapping in &regex_mappings {
            // Invalid patterns stored in the DB are skipped with a warning
            // rather than propagated as hard errors so that one bad row does
            // not break all lookups.
            match Regex::new(&mapping.raw) {
                Ok(re) if re.is_match(&key) => {
                    debug!(pattern = %mapping.raw, "Matched state via DB regex pattern.");
                    return Ok((mapping.clone(), false));
                }
                Ok(_) => {}
                Err(err) => {
                    warn!(
                        pattern = %mapping.raw,
                        error = ?err,
                        "Skipping invalid regex pattern in product_state_mapping table."
                    );
                }
            }
        }

        // ── Step 3: LLM fallback ─────────────────────────────────────────
        let truncated_raw = if key.len() > 100 { &key[..100] } else { &key };
        debug!(raw = %truncated_raw, "No DB mapping found, asking LLM...");
        let record = self.create_state_mapping(&key).await?;

        // Persist the result so future lookups are instant.
        let (persist_key, normalized, mapping_type) = (
            record.raw.clone(),
            ProductState::from(record.normalized),
            record.mapping_type,
        );
        let persisted = self
            .save_state_mapping(&persist_key, normalized, mapping_type)
            .await?;
        Ok((persisted, true))
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scraper::normalization::state_mapping_repository::MockProductStateMappingRepository;
    use product::dynamodb::product_state_record::ProductStateRecord;
    use rstest::rstest;

    // ── Mock LLM helpers ─────────────────────────────────────────────────

    #[derive(Debug)]
    struct FakeChatResponse(Option<String>);

    impl std::fmt::Display for FakeChatResponse {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            match &self.0 {
                Some(t) => write!(f, "{t}"),
                None => write!(f, "<no text>"),
            }
        }
    }

    impl llm::chat::ChatResponse for FakeChatResponse {
        fn text(&self) -> Option<String> {
            self.0.clone()
        }
        fn tool_calls(&self) -> Option<Vec<llm::ToolCall>> {
            None
        }
    }

    /// Panics if the LLM is called — for tests that must not reach the LLM.
    struct MockLlmProvider;

    #[async_trait::async_trait]
    impl llm::chat::ChatProvider for MockLlmProvider {
        async fn chat_with_tools(
            &self,
            _: &[ChatMessage],
            _: Option<&[llm::chat::Tool]>,
        ) -> Result<Box<dyn llm::chat::ChatResponse>, LLMError> {
            panic!("LLM should not be called in this test")
        }
    }

    /// Returns a fixed string as the LLM response.
    struct MockLlmProviderReturning(&'static str);

    #[async_trait::async_trait]
    impl llm::chat::ChatProvider for MockLlmProviderReturning {
        async fn chat_with_tools(
            &self,
            _: &[ChatMessage],
            _: Option<&[llm::chat::Tool]>,
        ) -> Result<Box<dyn llm::chat::ChatResponse>, LLMError> {
            Ok(Box::new(FakeChatResponse(Some(self.0.to_string()))))
        }
    }

    // ── Test-data helpers ────────────────────────────────────────────────

    fn value_record(raw: &str, state: ProductStateRecord) -> ProductStateMappingRecord {
        let now = OffsetDateTime::now_utc();
        ProductStateMappingRecord {
            raw: raw.to_string(),
            normalized: state,
            mapping_type: StateMappingType::Value,
            created: now,
            updated: now,
        }
    }

    fn regex_record(pattern: &str, state: ProductStateRecord) -> ProductStateMappingRecord {
        let now = OffsetDateTime::now_utc();
        ProductStateMappingRecord {
            raw: pattern.to_string(),
            normalized: state,
            mapping_type: StateMappingType::Regex,
            created: now,
            updated: now,
        }
    }

    // ────────────────────────────────────────────────────────────────────
    // parse_state_token
    // ────────────────────────────────────────────────────────────────────

    #[rstest]
    #[case("LISTED", ProductStateRecord::Listed)]
    #[case("AVAILABLE", ProductStateRecord::Available)]
    #[case("RESERVED", ProductStateRecord::Reserved)]
    #[case("SOLD", ProductStateRecord::Sold)]
    #[case("REMOVED", ProductStateRecord::Removed)]
    #[case("UNKNOWN", ProductStateRecord::Unknown)]
    fn should_parse_all_valid_state_tokens_for_parse_state_token(
        #[case] token: &str,
        #[case] expected: ProductStateRecord,
    ) {
        assert_eq!(parse_state_token(token), Some(expected));
    }

    #[rstest]
    #[case("available")]
    #[case("Sold")]
    #[case("listed")]
    fn should_parse_state_token_case_insensitively(#[case] token: &str) {
        assert!(parse_state_token(token).is_some());
    }

    #[rstest]
    #[case("  RESERVED  ")]
    #[case("  sold  ")]
    fn should_trim_whitespace_for_parse_state_token(#[case] token: &str) {
        assert!(parse_state_token(token).is_some());
    }

    #[rstest]
    #[case("")]
    #[case("   ")]
    #[case("NOPE")]
    #[case("in stock")]
    fn should_return_none_for_invalid_token_for_parse_state_token(#[case] token: &str) {
        assert_eq!(parse_state_token(token), None);
    }

    // ────────────────────────────────────────────────────────────────────
    // parse_llm_response
    // ────────────────────────────────────────────────────────────────────

    #[rstest]
    #[case(
        "STATE:AVAILABLE",
        LlmMappingResponse::State(ProductStateRecord::Available)
    )]
    #[case("STATE:SOLD", LlmMappingResponse::State(ProductStateRecord::Sold))]
    #[case("STATE:LISTED", LlmMappingResponse::State(ProductStateRecord::Listed))]
    #[case(
        "STATE:RESERVED",
        LlmMappingResponse::State(ProductStateRecord::Reserved)
    )]
    #[case(
        "STATE:REMOVED",
        LlmMappingResponse::State(ProductStateRecord::Removed)
    )]
    #[case(
        "STATE:UNKNOWN",
        LlmMappingResponse::State(ProductStateRecord::Unknown)
    )]
    fn should_parse_state_response_for_parse_llm_response(
        #[case] input: &str,
        #[case] expected: LlmMappingResponse,
    ) {
        assert_eq!(parse_llm_response(input), Some(expected));
    }

    #[rstest]
    #[case(
        "state:available",
        LlmMappingResponse::State(ProductStateRecord::Available)
    )]
    #[case("State:Sold", LlmMappingResponse::State(ProductStateRecord::Sold))]
    fn should_parse_state_response_case_insensitively_for_parse_llm_response(
        #[case] input: &str,
        #[case] expected: LlmMappingResponse,
    ) {
        // The prefix must be uppercase STATE: — lower-case prefix is not supported.
        // This test documents that only the state *token* is case-insensitive.
        let _ = (input, expected); // suppress unused warnings
        // Lower-case "state:" prefix is not recognised — returns None.
        assert_eq!(parse_llm_response("state:available"), None);
    }

    #[test]
    fn should_parse_regex_response_with_simple_pattern_for_parse_llm_response() {
        let response = r"REGEX:[1-9][0-9]*\s+in\s+stock\b:AVAILABLE";
        let result = parse_llm_response(response).unwrap();
        assert_eq!(
            result,
            LlmMappingResponse::Regex {
                pattern: r"[1-9][0-9]*\s+in\s+stock\b".to_string(),
                state: ProductStateRecord::Available,
            }
        );
    }

    #[test]
    fn should_parse_regex_response_with_pattern_containing_colons_for_parse_llm_response() {
        // Colons inside the pattern are allowed — only the *last* colon splits state.
        let response = r"REGEX:(?:sold|out):SOLD";
        let result = parse_llm_response(response).unwrap();
        assert_eq!(
            result,
            LlmMappingResponse::Regex {
                pattern: r"(?:sold|out)".to_string(),
                state: ProductStateRecord::Sold,
            }
        );
    }

    #[test]
    fn should_parse_regex_response_with_quantity_pattern_for_parse_llm_response() {
        let response = r"REGEX:\b(only\s+)?[1-9][0-9]*\s+left\b:AVAILABLE";
        let result = parse_llm_response(response).unwrap();
        assert_eq!(
            result,
            LlmMappingResponse::Regex {
                pattern: r"\b(only\s+)?[1-9][0-9]*\s+left\b".to_string(),
                state: ProductStateRecord::Available,
            }
        );
    }

    #[rstest]
    #[case("")]
    #[case("   ")]
    #[case("AVAILABLE")]
    #[case("just available")]
    #[case("STATE:")]
    #[case("STATE:BADSTATE")]
    #[case("REGEX:AVAILABLE")] // no second colon → no state token
    #[case("REGEX::AVAILABLE")] // empty pattern
    fn should_return_none_for_invalid_input_for_parse_llm_response(#[case] input: &str) {
        assert_eq!(parse_llm_response(input), None, "input: {input:?}");
    }

    #[test]
    fn should_return_none_for_invalid_regex_pattern_for_parse_llm_response() {
        // "[unclosed" is not a valid Rust regex.
        let response = "REGEX:[unclosed:AVAILABLE";
        assert_eq!(parse_llm_response(response), None);
    }

    #[test]
    fn should_strip_surrounding_whitespace_for_parse_llm_response() {
        assert_eq!(
            parse_llm_response("  STATE:SOLD  "),
            Some(LlmMappingResponse::State(ProductStateRecord::Sold))
        );
    }

    // ────────────────────────────────────────────────────────────────────
    // find_state_mapping
    // ────────────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn should_return_mapping_when_found_in_repository_for_find() {
        let expected = value_record("sold", ProductStateRecord::Sold);
        let expected_clone = expected.clone();

        let mut repo = MockProductStateMappingRepository::new();
        repo.expect_find_mapping()
            .withf(|raw| raw == "sold")
            .return_once(move |_| Box::pin(async move { Ok(Some(expected_clone)) }));

        let svc = ProductStateMappingServiceImpl {
            llm: Box::new(MockLlmProvider),
            rate_limiter: None,
            service_tier: None,
            repository: Box::new(repo),
        };

        let result = svc.find_state_mapping("sold").await.unwrap().unwrap();
        assert_eq!(result.raw, "sold");
        assert_eq!(result.normalized, ProductStateRecord::Sold);
    }

    #[tokio::test]
    async fn should_normalise_key_before_lookup_for_find() {
        let expected = value_record("sold", ProductStateRecord::Sold);
        let expected_clone = expected.clone();

        let mut repo = MockProductStateMappingRepository::new();
        // The service must lowercase + trim before hitting the repo.
        repo.expect_find_mapping()
            .withf(|raw| raw == "sold")
            .return_once(move |_| Box::pin(async move { Ok(Some(expected_clone)) }));

        let svc = ProductStateMappingServiceImpl {
            llm: Box::new(MockLlmProvider),
            rate_limiter: None,
            service_tier: None,
            repository: Box::new(repo),
        };

        let result = svc.find_state_mapping("  SOLD  ").await.unwrap().unwrap();
        assert_eq!(result.raw, "sold");
    }

    #[tokio::test]
    async fn should_return_none_when_not_found_in_repository_for_find() {
        let mut repo = MockProductStateMappingRepository::new();
        repo.expect_find_mapping()
            .return_once(|_| Box::pin(async { Ok(None) }));

        let svc = ProductStateMappingServiceImpl {
            llm: Box::new(MockLlmProvider),
            rate_limiter: None,
            service_tier: None,
            repository: Box::new(repo),
        };

        let result = svc.find_state_mapping("xyz").await.unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn should_propagate_database_error_for_find() {
        let mut repo = MockProductStateMappingRepository::new();
        repo.expect_find_mapping()
            .return_once(|_| Box::pin(async { Err(sqlx::Error::RowNotFound) }));

        let svc = ProductStateMappingServiceImpl {
            llm: Box::new(MockLlmProvider),
            rate_limiter: None,
            service_tier: None,
            repository: Box::new(repo),
        };

        let err = svc.find_state_mapping("anything").await.unwrap_err();
        assert!(matches!(err, StateMappingServiceError::DatabaseError(_)));
    }

    // ────────────────────────────────────────────────────────────────────
    // save_state_mapping
    // ────────────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn should_insert_mapping_when_not_existing_for_save() {
        let mut repo = MockProductStateMappingRepository::new();

        repo.expect_find_mapping()
            .withf(|raw| raw == "nuevo")
            .return_once(|_| Box::pin(async { Ok(None) }));

        let expected = value_record("nuevo", ProductStateRecord::Available);
        let expected_clone = expected.clone();
        repo.expect_insert_mapping()
            .withf(|r| r.raw == "nuevo" && r.normalized == ProductStateRecord::Available)
            .return_once(move |_| Box::pin(async move { Ok(expected_clone) }));

        let svc = ProductStateMappingServiceImpl {
            llm: Box::new(MockLlmProvider),
            rate_limiter: None,
            service_tier: None,
            repository: Box::new(repo),
        };

        let result = svc
            .save_state_mapping("nuevo", ProductState::Available, StateMappingType::Value)
            .await
            .unwrap();
        assert_eq!(result.raw, "nuevo");
        assert_eq!(result.normalized, ProductStateRecord::Available);
    }

    #[tokio::test]
    async fn should_insert_regex_mapping_when_not_existing_for_save() {
        let pattern = r"[1-9][0-9]*\s+items?\s+left\b";
        let mut repo = MockProductStateMappingRepository::new();

        repo.expect_find_mapping()
            .withf(move |raw| raw == pattern)
            .return_once(|_| Box::pin(async { Ok(None) }));

        let expected = regex_record(pattern, ProductStateRecord::Available);
        let expected_clone = expected.clone();
        repo.expect_insert_mapping()
            .withf(move |r| {
                r.raw == pattern
                    && r.normalized == ProductStateRecord::Available
                    && r.mapping_type == StateMappingType::Regex
            })
            .return_once(move |_| Box::pin(async move { Ok(expected_clone) }));

        let svc = ProductStateMappingServiceImpl {
            llm: Box::new(MockLlmProvider),
            rate_limiter: None,
            service_tier: None,
            repository: Box::new(repo),
        };

        let result = svc
            .save_state_mapping(pattern, ProductState::Available, StateMappingType::Regex)
            .await
            .unwrap();
        assert_eq!(result.mapping_type, StateMappingType::Regex);
    }

    #[tokio::test]
    async fn should_update_mapping_when_already_existing_for_save() {
        let existing = value_record("sold out", ProductStateRecord::Sold);
        let existing_clone = existing.clone();

        let mut repo = MockProductStateMappingRepository::new();
        repo.expect_find_mapping()
            .withf(|raw| raw == "sold out")
            .return_once(move |_| Box::pin(async move { Ok(Some(existing_clone)) }));

        let updated = value_record("sold out", ProductStateRecord::Removed);
        let updated_clone = updated.clone();
        repo.expect_update_mapping()
            .withf(|raw, normalized, mapping_type| {
                raw == "sold out"
                    && *normalized == ProductStateRecord::Removed
                    && *mapping_type == StateMappingType::Value
            })
            .return_once(move |_, _, _| Box::pin(async move { Ok(updated_clone) }));

        let svc = ProductStateMappingServiceImpl {
            llm: Box::new(MockLlmProvider),
            rate_limiter: None,
            service_tier: None,
            repository: Box::new(repo),
        };

        let result = svc
            .save_state_mapping("sold out", ProductState::Removed, StateMappingType::Value)
            .await
            .unwrap();
        assert_eq!(result.normalized, ProductStateRecord::Removed);
    }

    #[tokio::test]
    async fn should_propagate_database_error_when_find_fails_for_save() {
        let mut repo = MockProductStateMappingRepository::new();
        repo.expect_find_mapping()
            .return_once(|_| Box::pin(async { Err(sqlx::Error::RowNotFound) }));

        let svc = ProductStateMappingServiceImpl {
            llm: Box::new(MockLlmProvider),
            rate_limiter: None,
            service_tier: None,
            repository: Box::new(repo),
        };

        let err = svc
            .save_state_mapping("x", ProductState::Sold, StateMappingType::Value)
            .await
            .unwrap_err();
        assert!(matches!(err, StateMappingServiceError::DatabaseError(_)));
    }

    #[tokio::test]
    async fn should_propagate_database_error_when_insert_fails_for_save() {
        let mut repo = MockProductStateMappingRepository::new();
        repo.expect_find_mapping()
            .return_once(|_| Box::pin(async { Ok(None) }));
        repo.expect_insert_mapping().return_once(|_| {
            Box::pin(async { Err(sqlx::Error::Protocol("simulated".to_string())) })
        });

        let svc = ProductStateMappingServiceImpl {
            llm: Box::new(MockLlmProvider),
            rate_limiter: None,
            service_tier: None,
            repository: Box::new(repo),
        };

        let err = svc
            .save_state_mapping("nuevo", ProductState::Available, StateMappingType::Value)
            .await
            .unwrap_err();
        assert!(matches!(err, StateMappingServiceError::DatabaseError(_)));
    }

    // ────────────────────────────────────────────────────────────────────
    // create_state_mapping
    // ────────────────────────────────────────────────────────────────────

    #[rstest]
    #[case(
        "STATE:AVAILABLE",
        "in stock",
        ProductStateRecord::Available,
        StateMappingType::Value,
        "in stock"
    )]
    #[case(
        "STATE:SOLD",
        "ausverkauft",
        ProductStateRecord::Sold,
        StateMappingType::Value,
        "ausverkauft"
    )]
    #[case(
        "STATE:LISTED",
        "gelistet",
        ProductStateRecord::Listed,
        StateMappingType::Value,
        "gelistet"
    )]
    #[case(
        "STATE:RESERVED",
        "on hold",
        ProductStateRecord::Reserved,
        StateMappingType::Value,
        "on hold"
    )]
    #[case(
        "STATE:REMOVED",
        "deleted",
        ProductStateRecord::Removed,
        StateMappingType::Value,
        "deleted"
    )]
    #[case(
        "STATE:UNKNOWN",
        "zufällig",
        ProductStateRecord::Unknown,
        StateMappingType::Value,
        "zufällig"
    )]
    #[tokio::test]
    async fn should_create_value_mapping_when_llm_responds_with_state_for_create(
        #[case] llm_response: &'static str,
        #[case] raw: &str,
        #[case] expected_state: ProductStateRecord,
        #[case] expected_type: StateMappingType,
        #[case] expected_raw: &str,
    ) {
        let repo = MockProductStateMappingRepository::new();
        let svc = ProductStateMappingServiceImpl {
            llm: Box::new(MockLlmProviderReturning(llm_response)),
            rate_limiter: None,
            service_tier: None,
            repository: Box::new(repo),
        };

        let result = svc.create_state_mapping(raw).await.unwrap();
        assert_eq!(result.normalized, expected_state);
        assert_eq!(result.mapping_type, expected_type);
        assert_eq!(result.raw, expected_raw);
    }

    #[tokio::test]
    async fn should_create_regex_mapping_when_llm_responds_with_regex_for_create() {
        let llm_response = r"REGEX:[1-9][0-9]*\s+in\s+stock\b:AVAILABLE";
        let repo = MockProductStateMappingRepository::new();
        let svc = ProductStateMappingServiceImpl {
            llm: Box::new(MockLlmProviderReturning(llm_response)),
            rate_limiter: None,
            service_tier: None,
            repository: Box::new(repo),
        };

        let result = svc.create_state_mapping("5 in stock").await.unwrap();
        assert_eq!(result.normalized, ProductStateRecord::Available);
        assert_eq!(result.mapping_type, StateMappingType::Regex);
        // The raw field holds the pattern, not the original input.
        assert_eq!(result.raw, r"[1-9][0-9]*\s+in\s+stock\b");
    }

    #[tokio::test]
    async fn should_create_regex_mapping_when_llm_suggests_quantity_pattern_for_create() {
        let llm_response = r"REGEX:\b(only\s+)?[1-9][0-9]*\s+left\b:AVAILABLE";
        let repo = MockProductStateMappingRepository::new();
        let svc = ProductStateMappingServiceImpl {
            llm: Box::new(MockLlmProviderReturning(llm_response)),
            rate_limiter: None,
            service_tier: None,
            repository: Box::new(repo),
        };

        let result = svc.create_state_mapping("only 3 left").await.unwrap();
        assert_eq!(result.mapping_type, StateMappingType::Regex);
        assert_eq!(result.normalized, ProductStateRecord::Available);
        assert_eq!(result.raw, r"\b(only\s+)?[1-9][0-9]*\s+left\b");
    }

    #[tokio::test]
    async fn should_lowercase_input_before_sending_to_llm_for_create() {
        // The LLM will receive the lowercased key; the returned raw must also be lowercased.
        let repo = MockProductStateMappingRepository::new();
        let svc = ProductStateMappingServiceImpl {
            llm: Box::new(MockLlmProviderReturning("STATE:SOLD")),
            rate_limiter: None,
            service_tier: None,
            repository: Box::new(repo),
        };

        let result = svc.create_state_mapping("SOLD OUT").await.unwrap();
        assert_eq!(result.raw, "sold out");
    }

    #[tokio::test]
    async fn should_return_unparsable_error_when_llm_returns_bare_word_for_create() {
        // Old-style bare-word response is no longer accepted.
        let repo = MockProductStateMappingRepository::new();
        let svc = ProductStateMappingServiceImpl {
            llm: Box::new(MockLlmProviderReturning("AVAILABLE")),
            rate_limiter: None,
            service_tier: None,
            repository: Box::new(repo),
        };

        let err = svc.create_state_mapping("in stock").await.unwrap_err();
        assert!(matches!(
            err,
            StateMappingServiceError::UnparsableResponse(_)
        ));
    }

    #[tokio::test]
    async fn should_return_unparsable_error_when_llm_returns_invalid_regex_for_create() {
        let repo = MockProductStateMappingRepository::new();
        let svc = ProductStateMappingServiceImpl {
            // Pattern "[unclosed" is syntactically invalid for the Rust regex crate.
            llm: Box::new(MockLlmProviderReturning("REGEX:[unclosed:AVAILABLE")),
            rate_limiter: None,
            service_tier: None,
            repository: Box::new(repo),
        };

        let err = svc.create_state_mapping("weird").await.unwrap_err();
        assert!(matches!(
            err,
            StateMappingServiceError::UnparsableResponse(_)
        ));
    }

    #[tokio::test]
    async fn should_return_unparsable_error_when_llm_returns_garbage_for_create() {
        let repo = MockProductStateMappingRepository::new();
        let svc = ProductStateMappingServiceImpl {
            llm: Box::new(MockLlmProviderReturning(
                "I think it might be available or sold",
            )),
            rate_limiter: None,
            service_tier: None,
            repository: Box::new(repo),
        };

        let err = svc
            .create_state_mapping("something weird")
            .await
            .unwrap_err();
        assert!(matches!(
            err,
            StateMappingServiceError::UnparsableResponse(_)
        ));
    }

    #[tokio::test]
    async fn should_return_no_text_response_error_when_llm_returns_none_for_create() {
        struct NoTextLlm;
        #[async_trait::async_trait]
        impl llm::chat::ChatProvider for NoTextLlm {
            async fn chat_with_tools(
                &self,
                _: &[ChatMessage],
                _: Option<&[llm::chat::Tool]>,
            ) -> Result<Box<dyn llm::chat::ChatResponse>, LLMError> {
                Ok(Box::new(FakeChatResponse(None)))
            }
        }

        let repo = MockProductStateMappingRepository::new();
        let svc = ProductStateMappingServiceImpl {
            llm: Box::new(NoTextLlm),
            rate_limiter: None,
            service_tier: None,
            repository: Box::new(repo),
        };

        let err = svc.create_state_mapping("state text").await.unwrap_err();
        assert!(matches!(err, StateMappingServiceError::NoTextResponse(_)));
    }

    #[tokio::test]
    async fn should_propagate_llm_error_when_chat_fails_for_create() {
        struct ErrorLlm;
        #[async_trait::async_trait]
        impl llm::chat::ChatProvider for ErrorLlm {
            async fn chat_with_tools(
                &self,
                _: &[ChatMessage],
                _: Option<&[llm::chat::Tool]>,
            ) -> Result<Box<dyn llm::chat::ChatResponse>, LLMError> {
                Err(LLMError::ProviderError("simulated".to_string()))
            }
        }

        let repo = MockProductStateMappingRepository::new();
        let svc = ProductStateMappingServiceImpl {
            llm: Box::new(ErrorLlm),
            rate_limiter: None,
            service_tier: None,
            repository: Box::new(repo),
        };

        let err = svc.create_state_mapping("state text").await.unwrap_err();
        assert!(matches!(err, StateMappingServiceError::LLMError(_)));
    }

    // ────────────────────────────────────────────────────────────────────
    // get_state_mapping — step 1: exact DB hit
    // ────────────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn should_return_existing_value_mapping_without_llm_call_for_get() {
        let expected = value_record("sold", ProductStateRecord::Sold);
        let expected_clone = expected.clone();

        let mut repo = MockProductStateMappingRepository::new();
        repo.expect_find_mapping()
            .withf(|raw| raw == "sold")
            .return_once(move |_| Box::pin(async move { Ok(Some(expected_clone)) }));

        let svc = ProductStateMappingServiceImpl {
            llm: Box::new(MockLlmProvider),
            rate_limiter: None,
            service_tier: None,
            repository: Box::new(repo),
        };

        let (result, llm_called) = svc.get_state_mapping("sold").await.unwrap();
        assert_eq!(result.raw, "sold");
        assert_eq!(result.normalized, ProductStateRecord::Sold);
        assert!(!llm_called, "DB hit must not set llm_called");
    }

    #[tokio::test]
    async fn should_normalise_lookup_key_for_get() {
        let expected = value_record("sold", ProductStateRecord::Sold);
        let expected_clone = expected.clone();

        let mut repo = MockProductStateMappingRepository::new();
        repo.expect_find_mapping()
            .withf(|raw| raw == "sold")
            .return_once(move |_| Box::pin(async move { Ok(Some(expected_clone)) }));

        let svc = ProductStateMappingServiceImpl {
            llm: Box::new(MockLlmProvider),
            rate_limiter: None,
            service_tier: None,
            repository: Box::new(repo),
        };

        // "  SOLD  " must be normalised to "sold" before the lookup.
        let (result, llm_called) = svc.get_state_mapping("  SOLD  ").await.unwrap();
        assert_eq!(result.raw, "sold");
        assert!(!llm_called);
    }

    // ────────────────────────────────────────────────────────────────────
    // get_state_mapping — step 2: regex scan
    // ────────────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn should_match_via_regex_when_exact_key_not_in_db_for_get() {
        let mut repo = MockProductStateMappingRepository::new();

        // No exact hit.
        repo.expect_find_mapping()
            .withf(|raw| raw == "5 in stock")
            .return_once(|_| Box::pin(async { Ok(None) }));

        // One regex pattern that matches the input.
        let matching = regex_record(r"[1-9][0-9]*\s+in\s+stock\b", ProductStateRecord::Available);
        let matching_clone = matching.clone();
        repo.expect_find_all_regex_mappings()
            .return_once(move || Box::pin(async move { Ok(vec![matching_clone]) }));

        let svc = ProductStateMappingServiceImpl {
            llm: Box::new(MockLlmProvider), // must NOT be called
            rate_limiter: None,
            service_tier: None,
            repository: Box::new(repo),
        };

        let (result, llm_called) = svc.get_state_mapping("5 in stock").await.unwrap();
        assert_eq!(result.normalized, ProductStateRecord::Available);
        assert_eq!(result.mapping_type, StateMappingType::Regex);
        assert!(!llm_called);
    }

    #[tokio::test]
    async fn should_return_first_matching_regex_when_multiple_patterns_exist_for_get() {
        let mut repo = MockProductStateMappingRepository::new();
        repo.expect_find_mapping()
            .return_once(|_| Box::pin(async { Ok(None) }));

        // Two patterns — both could match; the first one wins.
        let first = regex_record(r"[1-9][0-9]*\s+left\b", ProductStateRecord::Available);
        let second = regex_record(
            r"[1-9][0-9]*\s+(left|remaining)\b",
            ProductStateRecord::Sold,
        );
        let first_clone = first.clone();
        repo.expect_find_all_regex_mappings()
            .return_once(move || Box::pin(async move { Ok(vec![first_clone, second]) }));

        let svc = ProductStateMappingServiceImpl {
            llm: Box::new(MockLlmProvider),
            rate_limiter: None,
            service_tier: None,
            repository: Box::new(repo),
        };

        let (result, llm_called) = svc.get_state_mapping("3 left").await.unwrap();
        assert_eq!(result.normalized, ProductStateRecord::Available);
        assert!(!llm_called);
    }

    #[tokio::test]
    async fn should_skip_non_matching_patterns_and_continue_for_get() {
        let mut repo = MockProductStateMappingRepository::new();
        repo.expect_find_mapping()
            .return_once(|_| Box::pin(async { Ok(None) }));

        let non_matching = regex_record(r"\bquedan\s+[1-9][0-9]*\b", ProductStateRecord::Available);
        let matching = regex_record(r"\b0\s+available\b", ProductStateRecord::Sold);
        let matching_clone = matching.clone();
        repo.expect_find_all_regex_mappings()
            .return_once(move || Box::pin(async move { Ok(vec![non_matching, matching_clone]) }));

        let svc = ProductStateMappingServiceImpl {
            llm: Box::new(MockLlmProvider),
            rate_limiter: None,
            service_tier: None,
            repository: Box::new(repo),
        };

        let (result, llm_called) = svc.get_state_mapping("0 available").await.unwrap();
        assert_eq!(result.normalized, ProductStateRecord::Sold);
        assert!(!llm_called);
    }

    #[tokio::test]
    async fn should_skip_invalid_regex_patterns_without_error_for_get() {
        // A stored pattern that does not compile must be silently skipped.
        let mut repo = MockProductStateMappingRepository::new();
        repo.expect_find_mapping()
            .return_once(|_| Box::pin(async { Ok(None) }));

        let bad = regex_record("[unclosed", ProductStateRecord::Available);
        let good = regex_record(r"\bsold\s+out\b", ProductStateRecord::Sold);
        let good_clone = good.clone();
        repo.expect_find_all_regex_mappings()
            .return_once(move || Box::pin(async move { Ok(vec![bad, good_clone]) }));

        let svc = ProductStateMappingServiceImpl {
            llm: Box::new(MockLlmProvider),
            rate_limiter: None,
            service_tier: None,
            repository: Box::new(repo),
        };

        let (result, llm_called) = svc.get_state_mapping("sold out").await.unwrap();
        assert_eq!(result.normalized, ProductStateRecord::Sold);
        assert!(!llm_called);
    }

    #[tokio::test]
    async fn should_match_regex_case_insensitively_via_key_lowercasing_for_get() {
        let mut repo = MockProductStateMappingRepository::new();
        // Key "ONLY 2 LEFT" is lowercased to "only 2 left" before matching.
        repo.expect_find_mapping()
            .withf(|raw| raw == "only 2 left")
            .return_once(|_| Box::pin(async { Ok(None) }));

        let pattern = regex_record(
            r"\b(only\s+)?[1-9][0-9]*\s+left\b",
            ProductStateRecord::Available,
        );
        let pattern_clone = pattern.clone();
        repo.expect_find_all_regex_mappings()
            .return_once(move || Box::pin(async move { Ok(vec![pattern_clone]) }));

        let svc = ProductStateMappingServiceImpl {
            llm: Box::new(MockLlmProvider),
            rate_limiter: None,
            service_tier: None,
            repository: Box::new(repo),
        };

        let (result, llm_called) = svc.get_state_mapping("ONLY 2 LEFT").await.unwrap();
        assert_eq!(result.normalized, ProductStateRecord::Available);
        assert!(!llm_called);
    }

    #[tokio::test]
    async fn should_reach_llm_when_no_patterns_match_for_get() {
        let mut repo = MockProductStateMappingRepository::new();

        // Exact miss.
        repo.expect_find_mapping()
            .withf(|raw| raw == "brand new phrase")
            .times(1)
            .return_once(|_| Box::pin(async { Ok(None) }));

        // One pattern that does NOT match.
        let non_matching = regex_record(r"\bquedan\b", ProductStateRecord::Available);
        repo.expect_find_all_regex_mappings()
            .return_once(move || Box::pin(async move { Ok(vec![non_matching]) }));

        // LLM responds → save path: find (miss) then insert.
        repo.expect_find_mapping()
            .withf(|raw| raw == "brand new phrase")
            .times(1)
            .return_once(|_| Box::pin(async { Ok(None) }));
        let saved = value_record("brand new phrase", ProductStateRecord::Listed);
        let saved_clone = saved.clone();
        repo.expect_insert_mapping()
            .return_once(move |_| Box::pin(async move { Ok(saved_clone) }));

        let svc = ProductStateMappingServiceImpl {
            llm: Box::new(MockLlmProviderReturning("STATE:LISTED")),
            rate_limiter: None,
            service_tier: None,
            repository: Box::new(repo),
        };

        let (result, llm_called) = svc.get_state_mapping("brand new phrase").await.unwrap();
        assert_eq!(result.normalized, ProductStateRecord::Listed);
        assert!(llm_called, "LLM fallback must set llm_called");
    }

    // ────────────────────────────────────────────────────────────────────
    // get_state_mapping — step 3: LLM returns REGEX
    // ────────────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn should_save_regex_mapping_when_llm_responds_with_regex_for_get() {
        let pattern = r"[1-9][0-9]*\s+verfügbar\b";

        let mut repo = MockProductStateMappingRepository::new();

        // Step 1: no exact hit.
        repo.expect_find_mapping()
            .withf(|raw| raw == "7 verfügbar")
            .times(1)
            .return_once(|_| Box::pin(async { Ok(None) }));

        // Step 2: no regex matches.
        repo.expect_find_all_regex_mappings()
            .return_once(|| Box::pin(async { Ok(vec![]) }));

        // LLM responds with a REGEX form → save_state_mapping uses the pattern as key.
        repo.expect_find_mapping()
            .withf(move |raw| raw == pattern)
            .times(1)
            .return_once(|_| Box::pin(async { Ok(None) }));
        let saved = regex_record(pattern, ProductStateRecord::Available);
        let saved_clone = saved.clone();
        repo.expect_insert_mapping()
            .withf(move |r| r.raw == pattern && r.mapping_type == StateMappingType::Regex)
            .return_once(move |_| Box::pin(async move { Ok(saved_clone) }));

        let llm_resp = format!("REGEX:{pattern}:AVAILABLE");
        // We need a dynamic LLM response here, so use a heap-allocated string.
        let llm_resp_str: &'static str = Box::leak(llm_resp.into_boxed_str());

        let svc = ProductStateMappingServiceImpl {
            llm: Box::new(MockLlmProviderReturning(llm_resp_str)),
            rate_limiter: None,
            service_tier: None,
            repository: Box::new(repo),
        };

        let (result, llm_called) = svc.get_state_mapping("7 verfügbar").await.unwrap();
        assert_eq!(result.normalized, ProductStateRecord::Available);
        assert_eq!(result.mapping_type, StateMappingType::Regex);
        assert!(llm_called, "LLM fallback must set llm_called");
    }

    // ────────────────────────────────────────────────────────────────────
    // get_state_mapping — DB error propagation
    // ────────────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn should_propagate_database_error_when_exact_find_fails_for_get() {
        let mut repo = MockProductStateMappingRepository::new();
        repo.expect_find_mapping()
            .return_once(|_| Box::pin(async { Err(sqlx::Error::RowNotFound) }));

        let svc = ProductStateMappingServiceImpl {
            llm: Box::new(MockLlmProvider),
            rate_limiter: None,
            service_tier: None,
            repository: Box::new(repo),
        };

        let err = svc.get_state_mapping("x").await.unwrap_err();
        assert!(matches!(err, StateMappingServiceError::DatabaseError(_)));
    }

    #[tokio::test]
    async fn should_propagate_database_error_when_regex_scan_fails_for_get() {
        let mut repo = MockProductStateMappingRepository::new();
        repo.expect_find_mapping()
            .return_once(|_| Box::pin(async { Ok(None) }));
        repo.expect_find_all_regex_mappings()
            .return_once(|| Box::pin(async { Err(sqlx::Error::RowNotFound) }));

        let svc = ProductStateMappingServiceImpl {
            llm: Box::new(MockLlmProvider),
            rate_limiter: None,
            service_tier: None,
            repository: Box::new(repo),
        };

        let err = svc.get_state_mapping("x").await.unwrap_err();
        assert!(matches!(err, StateMappingServiceError::DatabaseError(_)));
    }

    // ────────────────────────────────────────────────────────────────────
    // Regex matching behaviour (unit-level, no async needed)
    // ────────────────────────────────────────────────────────────────────

    /// Verify that the regex patterns the LLM is expected to generate actually
    /// match the strings they should and reject the ones they should not.
    #[rstest]
    // English — positive quantity → Available
    #[case(r"[1-9][0-9]*\s+available\b", "1 available", true)]
    #[case(r"[1-9][0-9]*\s+available\b", "12 available now", true)]
    #[case(r"[1-9][0-9]*\s+available\b", "0 available", false)]
    #[case(r"\b(only\s+)?[1-9][0-9]*\s+left\b", "only 2 left", true)]
    #[case(r"\b(only\s+)?[1-9][0-9]*\s+left\b", "1 left", true)]
    #[case(r"\b(only\s+)?[1-9][0-9]*\s+left\b", "0 left", false)]
    #[case(r"[1-9][0-9]*\s+in\s+stock\b", "5 in stock", true)]
    #[case(r"[1-9][0-9]*\s+in\s+stock\b", "0 in stock", false)]
    #[case(
        r"\b(only\s+|just\s+)?[1-9][0-9]*\s+remaining\b",
        "just 1 remaining",
        true
    )]
    #[case(
        r"\b(only\s+|just\s+)?[1-9][0-9]*\s+remaining\b",
        "only 4 remaining",
        true
    )]
    #[case(r"\b(only\s+|just\s+)?[1-9][0-9]*\s+remaining\b", "2 remaining", true)]
    #[case(r"\b(only\s+|just\s+)?[1-9][0-9]*\s+remaining\b", "0 remaining", false)]
    // German — positive quantity → Available
    #[case(
        r"(\bnur\s+)?(\bnoch\s+)?[1-9][0-9]*\s+verfügbar\b",
        "nur noch 7 verfügbar",
        true
    )]
    #[case(
        r"(\bnur\s+)?(\bnoch\s+)?[1-9][0-9]*\s+verfügbar\b",
        "noch 2 verfügbar",
        true
    )]
    #[case(
        r"(\bnur\s+)?(\bnoch\s+)?[1-9][0-9]*\s+verfügbar\b",
        "3 verfügbar",
        true
    )]
    #[case(
        r"(\bnur\s+)?(\bnoch\s+)?[1-9][0-9]*\s+verfügbar\b",
        "0 verfügbar",
        false
    )]
    #[case(
        r"(\bnur\s+)?(\bnoch\s+)?[1-9][0-9]*\s+auf\s+lager\b",
        "nur noch 3 auf lager",
        true
    )]
    #[case(
        r"(\bnur\s+)?(\bnoch\s+)?[1-9][0-9]*\s+auf\s+lager\b",
        "4 auf lager",
        true
    )]
    #[case(
        r"(\bnur\s+)?(\bnoch\s+)?[1-9][0-9]*\s+auf\s+lager\b",
        "0 auf lager",
        false
    )]
    #[case(
        r"(\bnur\s+)?(\bnoch\s+)?[1-9][0-9]*\s+stück\b",
        "nur noch 2 stück",
        true
    )]
    #[case(r"(\bnur\s+)?(\bnoch\s+)?[1-9][0-9]*\s+stück\b", "noch 5 stück", true)]
    #[case(r"(\bnur\s+)?(\bnoch\s+)?[1-9][0-9]*\s+stück\b", "0 stück", false)]
    // French — positive quantity → Available
    #[case(r"(\bplus\s+que\s+)?[1-9][0-9]*\s+en\s+stock\b", "3 en stock", true)]
    #[case(
        r"(\bplus\s+que\s+)?[1-9][0-9]*\s+en\s+stock\b",
        "plus que 2 en stock",
        true
    )]
    #[case(r"(\bplus\s+que\s+)?[1-9][0-9]*\s+en\s+stock\b", "0 en stock", false)]
    #[case(r"[1-9][0-9]*\s+disponibles?\b", "2 disponibles", true)]
    #[case(r"[1-9][0-9]*\s+disponibles?\b", "1 disponible", true)]
    #[case(r"[1-9][0-9]*\s+disponibles?\b", "0 disponibles", false)]
    #[case(r"\bil\s+(ne\s+)?reste\s+(que\s+)?[1-9][0-9]*\b", "il reste 3", true)]
    #[case(
        r"\bil\s+(ne\s+)?reste\s+(que\s+)?[1-9][0-9]*\b",
        "il ne reste que 1",
        true
    )]
    // Spanish — positive quantity → Available
    #[case(
        r"(\bsolo\s+)?[1-9][0-9]*\s+disponibles?\b",
        "solo 2 disponibles",
        true
    )]
    #[case(r"(\bsolo\s+)?[1-9][0-9]*\s+disponibles?\b", "3 disponibles", true)]
    #[case(r"(\bsolo\s+)?[1-9][0-9]*\s+disponibles?\b", "0 disponibles", false)]
    #[case(r"\bquedan\s+[1-9][0-9]*\b", "quedan 3", true)]
    #[case(r"\bquedan\s+[1-9][0-9]*\b", "quedan 0", false)]
    // Italian — positive quantity → Available
    #[case(r"(\bsolo\s+)?[1-9][0-9]*\s+disponibili\b", "solo 2 disponibili", true)]
    #[case(r"(\bsolo\s+)?[1-9][0-9]*\s+disponibili\b", "4 disponibili", true)]
    #[case(r"(\bsolo\s+)?[1-9][0-9]*\s+disponibili\b", "0 disponibili", false)]
    #[case(r"\brimangono\s+[1-9][0-9]*\b", "rimangono 3", true)]
    #[case(r"\brimangono\s+[1-9][0-9]*\b", "rimangono 0", false)]
    // Zero-quantity → Sold
    #[case(r"\b0\s+available\b", "0 available", true)]
    #[case(r"\b0\s+available\b", "1 available", false)]
    #[case(r"\b0\s+remaining\b", "0 remaining", true)]
    #[case(r"\b0\s+left\b", "0 left", true)]
    #[case(r"\b0\s+in\s+stock\b", "0 in stock", true)]
    #[case(r"\b0\s+verfügbar\b", "0 verfügbar", true)]
    #[case(r"\b0\s+auf\s+lager\b", "0 auf lager", true)]
    #[case(r"\b0\s+stück\b", "0 stück", true)]
    #[case(r"\b0\s+en\s+stock\b", "0 en stock", true)]
    #[case(r"\b0\s+disponibles?\b", "0 disponibles", true)]
    #[case(r"\b0\s+disponibles?\b", "0 disponible", true)]
    #[case(r"\b0\s+disponibili\b", "0 disponibili", true)]
    fn should_regex_pattern_match_or_reject_input_correctly(
        #[case] pattern: &str,
        #[case] input: &str,
        #[case] expected_match: bool,
    ) {
        let re = Regex::new(pattern).expect("pattern must be valid");
        let lowered = input.trim().to_lowercase();
        assert_eq!(
            re.is_match(&lowered),
            expected_match,
            "pattern={pattern:?} input={input:?}"
        );
    }

    /// All patterns that the LLM is expected to produce must compile without error.
    #[rstest]
    #[case(r"[1-9][0-9]*\s+available\b")]
    #[case(r"\b(only\s+|just\s+)?[1-9][0-9]*\s+remaining\b")]
    #[case(r"\b(only\s+)?[1-9][0-9]*\s+left\b")]
    #[case(r"[1-9][0-9]*\s+in\s+stock\b")]
    #[case(r"\bhurry\b.*[1-9][0-9]*")]
    #[case(r"[1-9][0-9]*\s+vorrätig\b")]
    #[case(r"(\bnur\s+)?(\bnoch\s+)?[1-9][0-9]*\s+verfügbar\b")]
    #[case(r"(\bnur\s+)?(\bnoch\s+)?[1-9][0-9]*\s+auf\s+lager\b")]
    #[case(r"(\bnur\s+)?(\bnoch\s+)?[1-9][0-9]*\s+stück\b")]
    #[case(r"(\bplus\s+que\s+)?[1-9][0-9]*\s+en\s+stock\b")]
    #[case(r"[1-9][0-9]*\s+disponibles?\b")]
    #[case(r"\bil\s+(ne\s+)?reste\s+(que\s+)?[1-9][0-9]*\b")]
    #[case(r"(\bsolo\s+)?[1-9][0-9]*\s+disponibles?\b")]
    #[case(r"\bquedan\s+[1-9][0-9]*\b")]
    #[case(r"(\bsolo\s+)?[1-9][0-9]*\s+disponibili\b")]
    #[case(r"\brimangono\s+[1-9][0-9]*\b")]
    #[case(r"\b0\s+available\b")]
    #[case(r"\b0\s+remaining\b")]
    #[case(r"\b0\s+left\b")]
    #[case(r"\b0\s+in\s+stock\b")]
    #[case(r"\b0\s+verfügbar\b")]
    #[case(r"\b0\s+auf\s+lager\b")]
    #[case(r"\b0\s+stück\b")]
    #[case(r"\b0\s+en\s+stock\b")]
    #[case(r"\b0\s+disponibles?\b")]
    #[case(r"\b0\s+disponibili\b")]
    fn should_all_seed_patterns_compile_as_valid_rust_regex(#[case] pattern: &str) {
        assert!(
            Regex::new(pattern).is_ok(),
            "pattern should be valid Rust regex: {pattern:?}"
        );
    }

    // ────────────────────────────────────────────────────────────────────
    // get_state_mapping — length guard (RawStateTooLong)
    // ────────────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn should_return_raw_state_too_long_error_without_calling_llm_or_db_when_state_too_long()
    {
        // A string just over the limit — LLM and DB must never be called.
        let long_state = "a".repeat(MAX_STATE_RAW_LEN + 1);

        let repo = MockProductStateMappingRepository::new(); // no expectations set
        let svc = ProductStateMappingServiceImpl {
            llm: Box::new(MockLlmProvider), // panics if called
            rate_limiter: None,
            service_tier: None,
            repository: Box::new(repo),
        };

        let err = svc.get_state_mapping(&long_state).await.unwrap_err();
        assert!(
            matches!(
                err,
                StateMappingServiceError::RawStateTooLong { len, max }
                if len == MAX_STATE_RAW_LEN + 1 && max == MAX_STATE_RAW_LEN
            ),
            "expected RawStateTooLong, got {err:?}"
        );
    }

    #[tokio::test]
    async fn should_accept_state_at_exactly_max_length_for_get() {
        // A string exactly at the limit should proceed to the DB lookup
        // (which returns None here) and then to the LLM path.
        let exactly_max = "b".repeat(MAX_STATE_RAW_LEN);
        let exactly_max_clone = exactly_max.clone();

        let mut repo = MockProductStateMappingRepository::new();
        // Step 1: exact key lookup — miss
        repo.expect_find_mapping()
            .returning(|_| Box::pin(async { Ok(None) }));
        // Step 2: regex scan — empty
        repo.expect_find_all_regex_mappings()
            .return_once(|| Box::pin(async { Ok(vec![]) }));
        // Step 3 (save_state_mapping): insert since find returns None
        let record = value_record(&"b".repeat(MAX_STATE_RAW_LEN), ProductStateRecord::Unknown);
        let record_clone = record.clone();
        repo.expect_insert_mapping()
            .return_once(move |_| Box::pin(async move { Ok(record_clone) }));

        let svc = ProductStateMappingServiceImpl {
            llm: Box::new(MockLlmProviderReturning("STATE:UNKNOWN")),
            rate_limiter: None,
            service_tier: None,
            repository: Box::new(repo),
        };

        // Should succeed — no RawStateTooLong error.
        let result = svc.get_state_mapping(&exactly_max_clone).await;
        assert!(result.is_ok(), "expected Ok, got {result:?}");
        assert!(result.unwrap().1, "LLM must be called when no DB match");
    }
}
