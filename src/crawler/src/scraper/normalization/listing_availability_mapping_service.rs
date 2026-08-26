use crate::llm_runtime::{
    CrawlerLlmGovernor, ValidatedGenerationError, generate_validated_with_governor,
};
use crate::scraper::normalization::listing_availability_mapping::{
    ListingAvailabilityDecisionKind, ListingAvailabilityMapping, ListingAvailabilityMappingRecord,
    ListingAvailabilityMappingType,
};
use crate::scraper::normalization::listing_availability_mapping_repository::ListingAvailabilityMappingRepository;
use large_language_model::{
    GenerationOptions, LargeLanguageModel, LargeLanguageModelError, LlmOperation,
    StructuredGenerationRequest,
};
use product_listing_core::listing_availability::ListingAvailability;
use regex::Regex;
use schemars::{JsonSchema, schema_for};
use serde::{Deserialize, Serialize};
use std::{sync::Arc, time::Duration};
use time::OffsetDateTime;
use tracing::warn;

pub const MAX_AVAILABILITY_RAW_LEN: usize = 512;

const LISTING_AVAILABILITY_MAPPING_SYSTEM_INSTRUCTION: &str = "\
You classify scraped product availability text for an antiques marketplace.\n\
Return one mapping result:\n\n\
  AVAILABILITY — the text makes a clear availability assertion; provide the canonical availability value.\n\
  NO_ASSERTION — the successfully parsed product page has no availability assertion.\n\
  IGNORE — the extraction is ambiguous, unsupported, or not availability evidence. Never use this for page removal.\n\
Removal requires verified HTTP 404/410, a verified redirect, or a verified removed-page schema.\n\
Choose a direct mapping for a stable phrase. Choose a regex only for a valid Rust regex pattern.\n\
Regexes match trimmed, lower-cased input. Return only JSON matching the response schema.";

#[derive(Debug, thiserror::Error)]
pub enum ListingAvailabilityMappingServiceError {
    #[error(transparent)]
    LargeLanguageModelError(#[from] LargeLanguageModelError),
    #[error("LLM returned an invalid listing availability mapping response")]
    UnparsableResponse,
    #[error("failed to serialize listing availability mapping response JSON schema")]
    ResponseJsonSchemaSerialization(#[source] serde_json::Error),
    #[error("database error: {0}")]
    DatabaseError(#[from] sqlx::Error),
    #[error("database error after listing availability mapping LLM call: {0}")]
    DatabaseErrorAfterLlm(#[source] sqlx::Error),
    #[error("availability text too long ({len} bytes, max {max})")]
    RawStateTooLong { len: usize, max: usize },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
enum LlmListingAvailability {
    Available,
    InStock,
    LimitedAvailability,
    BackOrder,
    MadeToOrder,
    PreOrder,
    PreSale,
    Unavailable,
    Reserved,
    OutOfStock,
    SoldOut,
}

impl LlmListingAvailability {
    const fn into_core(self) -> ListingAvailability {
        match self {
            Self::Available => ListingAvailability::Available,
            Self::InStock => ListingAvailability::InStock,
            Self::LimitedAvailability => ListingAvailability::LimitedAvailability,
            Self::BackOrder => ListingAvailability::BackOrder,
            Self::MadeToOrder => ListingAvailability::MadeToOrder,
            Self::PreOrder => ListingAvailability::PreOrder,
            Self::PreSale => ListingAvailability::PreSale,
            Self::Unavailable => ListingAvailability::Unavailable,
            Self::Reserved => ListingAvailability::Reserved,
            Self::OutOfStock => ListingAvailability::OutOfStock,
            Self::SoldOut => ListingAvailability::SoldOut,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "mapping", rename_all = "SCREAMING_SNAKE_CASE")]
enum LlmMappingResponse {
    Availability {
        availability: LlmListingAvailability,
    },
    NoAssertion,
    Ignore,
    Regex {
        pattern: String,
        result: LlmAvailabilityMappingResult,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "mapping", rename_all = "SCREAMING_SNAKE_CASE")]
enum LlmAvailabilityMappingResult {
    Availability {
        availability: LlmListingAvailability,
    },
    NoAssertion,
    Ignore,
}

impl From<LlmAvailabilityMappingResult> for ListingAvailabilityMapping {
    fn from(value: LlmAvailabilityMappingResult) -> Self {
        match value {
            LlmAvailabilityMappingResult::Availability { availability } => {
                Self::Availability(availability.into_core())
            }
            LlmAvailabilityMappingResult::NoAssertion => Self::NoAssertion,
            LlmAvailabilityMappingResult::Ignore => Self::Ignore,
        }
    }
}

#[derive(Debug, PartialEq)]
enum ParsedLlmMappingResponse {
    Value(ListingAvailabilityMapping),
    Regex {
        pattern: String,
        mapping: ListingAvailabilityMapping,
    },
}

fn validate_llm_response(response: LlmMappingResponse) -> Result<ParsedLlmMappingResponse, ()> {
    match response {
        LlmMappingResponse::Availability { availability } => Ok(ParsedLlmMappingResponse::Value(
            ListingAvailabilityMapping::Availability(availability.into_core()),
        )),
        LlmMappingResponse::NoAssertion => Ok(ParsedLlmMappingResponse::Value(
            ListingAvailabilityMapping::NoAssertion,
        )),
        LlmMappingResponse::Ignore => Ok(ParsedLlmMappingResponse::Value(
            ListingAvailabilityMapping::Ignore,
        )),
        LlmMappingResponse::Regex { pattern, result }
            if !pattern.trim().is_empty() && Regex::new(&pattern).is_ok() =>
        {
            Ok(ParsedLlmMappingResponse::Regex {
                pattern,
                mapping: result.into(),
            })
        }
        LlmMappingResponse::Regex { .. } => Err(()),
    }
}

fn mapping_error(error: ValidatedGenerationError<()>) -> ListingAvailabilityMappingServiceError {
    match error {
        ValidatedGenerationError::Model(error) => {
            ListingAvailabilityMappingServiceError::LargeLanguageModelError(error)
        }
        ValidatedGenerationError::Validation(()) => {
            ListingAvailabilityMappingServiceError::UnparsableResponse
        }
    }
}

fn request(
    raw: String,
) -> Result<StructuredGenerationRequest, ListingAvailabilityMappingServiceError> {
    Ok(StructuredGenerationRequest {
        operation: LlmOperation::CrawlerAvailabilityMapping,
        system_instruction: LISTING_AVAILABILITY_MAPPING_SYSTEM_INSTRUCTION.to_owned(),
        prompt: raw,
        image_urls: Vec::new(),
        response_json_schema: serde_json::to_value(schema_for!(LlmMappingResponse))
            .map_err(ListingAvailabilityMappingServiceError::ResponseJsonSchemaSerialization)?,
        options: GenerationOptions {
            temperature: 0.0,
            max_output_tokens: 256,
            request_timeout: Duration::from_secs(60),
        },
    })
}

#[async_trait::async_trait]
#[mockall::automock]
pub trait ListingAvailabilityMappingService: Send + Sync {
    async fn get_listing_availability_mapping(
        &self,
        raw: &str,
    ) -> Result<(ListingAvailabilityMapping, bool), ListingAvailabilityMappingServiceError>;
}

pub struct ListingAvailabilityMappingServiceImpl<Llm> {
    llm: Llm,
    governor: Option<Arc<CrawlerLlmGovernor>>,
    repository: Box<dyn ListingAvailabilityMappingRepository + Send + Sync>,
}

impl<Llm> ListingAvailabilityMappingServiceImpl<Llm>
where
    Llm: LargeLanguageModel,
{
    pub fn new(
        llm: Llm,
        repository: Box<dyn ListingAvailabilityMappingRepository + Send + Sync>,
        governor: Option<Arc<CrawlerLlmGovernor>>,
    ) -> Self {
        Self {
            llm,
            governor,
            repository,
        }
    }

    async fn persist(
        &self,
        raw: String,
        mapping_type: ListingAvailabilityMappingType,
        mapping: ListingAvailabilityMapping,
    ) -> Result<(), ListingAvailabilityMappingServiceError> {
        let decision_kind = ListingAvailabilityDecisionKind::from_mapping(mapping)
            .ok_or(ListingAvailabilityMappingServiceError::UnparsableResponse)?;
        let now = OffsetDateTime::now_utc();
        let record = ListingAvailabilityMappingRecord {
            raw: raw.clone(),
            availability: mapping.availability(),
            mapping_type,
            decision_kind,
            created: now,
            updated: now,
        };
        if self.repository.find_mapping(&raw).await?.is_some() {
            self.repository.update_mapping(&record).await?;
        } else {
            self.repository.insert_mapping(&record).await?;
        }
        Ok(())
    }
}

#[async_trait::async_trait]
impl<Llm> ListingAvailabilityMappingService for ListingAvailabilityMappingServiceImpl<Llm>
where
    Llm: LargeLanguageModel,
{
    async fn get_listing_availability_mapping(
        &self,
        raw: &str,
    ) -> Result<(ListingAvailabilityMapping, bool), ListingAvailabilityMappingServiceError> {
        let raw = raw.trim().to_lowercase();
        if raw.len() > MAX_AVAILABILITY_RAW_LEN {
            return Err(ListingAvailabilityMappingServiceError::RawStateTooLong {
                len: raw.len(),
                max: MAX_AVAILABILITY_RAW_LEN,
            });
        }

        if let Some(mapping) =
            crate::scraper::normalization::schema_org_availability::map_schema_org_availability(
                &raw,
            )
        {
            return Ok((mapping, false));
        }
        if let Some(record) = self.repository.find_mapping(&raw).await? {
            return Ok((record.mapping(), false));
        }
        for record in self.repository.find_all_regex_mappings().await? {
            match Regex::new(&record.raw) {
                Ok(regex) if regex.is_match(&raw) => return Ok((record.mapping(), false)),
                Ok(_) => {}
                Err(_) => warn!(
                    error_kind = "invalid_regex",
                    "Skipping invalid listing availability regex mapping"
                ),
            }
        }

        let parsed = generate_validated_with_governor::<
            _,
            LlmMappingResponse,
            ParsedLlmMappingResponse,
            (),
            _,
            _,
        >(
            &self.llm,
            self.governor.as_ref(),
            request(raw.clone())?,
            3,
            validate_llm_response,
            |_| "invalid_mapping",
        )
        .await
        .map_err(mapping_error)?;
        let (stored_raw, mapping_type, mapping) = match parsed {
            ParsedLlmMappingResponse::Value(mapping) => {
                (raw, ListingAvailabilityMappingType::Value, mapping)
            }
            ParsedLlmMappingResponse::Regex { pattern, mapping } => {
                (pattern, ListingAvailabilityMappingType::Regex, mapping)
            }
        };
        if mapping.is_persistable() {
            self.persist(stored_raw, mapping_type, mapping)
                .await
                .map_err(|error| match error {
                    ListingAvailabilityMappingServiceError::DatabaseError(source) => {
                        ListingAvailabilityMappingServiceError::DatabaseErrorAfterLlm(source)
                    }
                    other => other,
                })?;
        }
        Ok((mapping, true))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_validate_no_assertion_and_ignore_as_boundary_decisions() {
        assert!(matches!(
            validate_llm_response(LlmMappingResponse::NoAssertion),
            Ok(ParsedLlmMappingResponse::Value(
                ListingAvailabilityMapping::NoAssertion
            ))
        ));
        assert!(matches!(
            validate_llm_response(LlmMappingResponse::Ignore),
            Ok(ParsedLlmMappingResponse::Value(
                ListingAvailabilityMapping::Ignore
            ))
        ));
    }
}
