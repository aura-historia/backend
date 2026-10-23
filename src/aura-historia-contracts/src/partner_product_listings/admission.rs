//! Shared ingestion admission parsing and truthful queue outcomes. No transport framing.
use crate::product_listing_management::MutationInputError;
use product_listing_core::source_listing_id::SourceListingId;
use product_listing_service::use_cases::{
    IndexedProductListingIngestionIntent, ProductListingIngestionIdempotencyKey,
    ProductListingIngestionIntent, ProductListingIngestionItemOutcome,
    ProductListingIngestionNotAttemptedReason, ProductListingIngestionOutcome,
    ProductListingIngestionRejectionReason, ProductListingIngestionSubmissionResult,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::value::RawValue;
use std::marker::PhantomData;

pub fn parse_idempotency_key(
    value: &str,
) -> Result<ProductListingIngestionIdempotencyKey, MutationInputError> {
    if value.contains(',') {
        return Err(MutationInputError::invalid(
            "idempotencyKey",
            "Idempotency key must contain 1 to 128 visible ASCII bytes excluding comma.",
        ));
    }
    ProductListingIngestionIdempotencyKey::new(value).map_err(|error| {
        MutationInputError::caused(
            "idempotencyKey",
            "Idempotency key must contain 1 to 128 visible ASCII bytes excluding comma.",
            error,
        )
    })
}

pub fn optional_idempotency_key_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
    serde_json::json!({"anyOf":[{"type":"string","minLength":1,"maxLength":128,"pattern":"^[!-+\\--~]+$"},{"type":"null"}]}).try_into().expect("static schema")
}

/// Decode each item only after the complete JSON envelope is valid. T supplies the shared schema.
pub struct RawAdmissionItems<T> {
    raw: Vec<Box<RawValue>>,
    item: PhantomData<T>,
}
impl<T> RawAdmissionItems<T> {
    pub fn into_raw(self) -> Vec<Box<RawValue>> {
        self.raw
    }
}
impl<'de, T> Deserialize<'de> for RawAdmissionItems<T> {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = Vec::<Box<RawValue>>::deserialize(deserializer)?;
        super::validate_batch_size(raw.len()).map_err(serde::de::Error::custom)?;
        Ok(Self {
            raw,
            item: PhantomData,
        })
    }
}
impl<T: JsonSchema> JsonSchema for RawAdmissionItems<T> {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        format!("AdmissionItems_{}", T::schema_name()).into()
    }
    fn json_schema(generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
        let mut schema = <Vec<T>>::json_schema(generator);
        schema.insert(
            "maxItems".into(),
            serde_json::json!(super::MAX_PARTNER_PRODUCT_LISTING_BATCH_SIZE),
        );
        schema
    }
}

pub fn safe_source_id(value: &str) -> Option<String> {
    if value.len() > 128 || value.chars().any(char::is_control) {
        return None;
    }
    SourceListingId::try_from(value)
        .ok()
        .map(|id| id.to_string())
}

pub fn parse_envelope(body: &str) -> Result<Vec<Box<RawValue>>, MutationInputError> {
    if body.trim().is_empty() {
        return Err(MutationInputError::invalid(
            "products",
            "Body cannot be empty.",
        ));
    }
    // Parse the *entire* body before any sends, retaining each item's exact JSON representation.
    // Parsing through Value would discard duplicate fields before strict DTO deserialization.
    let items: Vec<Box<RawValue>> = serde_json::from_str(body)
        .map_err(|error| MutationInputError::caused("products", "Expected a JSON array.", error))?;
    super::validate_batch_size(items.len())?;
    Ok(items)
}

pub fn parse_items<T: DeserializeOwned>(
    raw_items: Vec<Box<RawValue>>,
    source_id: impl Fn(&T) -> &str,
    into_intent: impl Fn(T) -> Result<ProductListingIngestionIntent, MutationInputError>,
) -> (
    Vec<IndexedProductListingIngestionIntent>,
    Vec<Option<String>>,
    Vec<BatchFailure>,
) {
    let mut items = Vec::with_capacity(raw_items.len());
    let mut source_ids = vec![None; raw_items.len()];
    let mut failures = Vec::new();
    for (index, raw) in raw_items.into_iter().enumerate() {
        let product: T = match serde_json::from_str(raw.get()) {
            Ok(product) => product,
            Err(_) => {
                failures.push(BatchFailure::invalid(index, None, "BAD_BODY_VALUE"));
                continue;
            }
        };
        // Index is authoritative; echo only a short, printable, valid listing key.
        source_ids[index] = safe_source_id(source_id(&product));
        match into_intent(product) {
            Ok(intent) => items.push(IndexedProductListingIngestionIntent { index, intent }),
            Err(error) => failures.push(BatchFailure::invalid(
                index,
                source_ids[index].clone(),
                if error.invalid_object_id {
                    "INVALID_OBJECT_ID"
                } else {
                    "BAD_BODY_VALUE"
                },
            )),
        }
    }
    (items, source_ids, failures)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FailureKind {
    Invalid,
    MessageSize,
    Send,
    Internal,
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct BatchFailure {
    #[schemars(range(max = u64::MAX))]
    pub index: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_listing_id: Option<String>,
    pub error: &'static str,
    pub retryable: bool,
    #[serde(skip)]
    pub kind: FailureKind,
}

impl BatchFailure {
    pub fn invalid(index: usize, source_listing_id: Option<String>, error: &'static str) -> Self {
        Self {
            index,
            source_listing_id,
            error,
            retryable: false,
            kind: FailureKind::Invalid,
        }
    }
}

#[derive(Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct BatchReport {
    pub submission_id: String,
    #[schemars(range(max = u64::MAX))]
    pub accepted_count: usize,
    pub failures: Vec<BatchFailure>,
}

#[derive(Clone, Copy, Debug, Serialize, JsonSchema)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum AdmissionOutcomeData {
    Published,
    Rejected,
    NotAttempted,
    Unknown,
}

#[derive(Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct AdmissionItemData {
    #[schemars(range(max = u64::MAX))]
    pub index: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_listing_id: Option<String>,
    pub outcome: AdmissionOutcomeData,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<&'static str>,
    pub retryable: bool,
}

/// MCP includes the effective retry key and a disposition for every original position.
/// REST retains its established body by emitting only `report` and its key header.
#[derive(Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct AdmissionReportData {
    #[serde(flatten)]
    pub report: BatchReport,
    pub idempotency_key: String,
    #[schemars(range(max = u64::MAX))]
    pub original_input_count: usize,
    pub items: Vec<AdmissionItemData>,
}

pub fn report(
    result: ProductListingIngestionSubmissionResult,
    mut local_failures: Vec<BatchFailure>,
    source_ids: &[Option<String>],
) -> AdmissionReportData {
    let mut accepted = 0;
    let mut seen = vec![false; result.original_input_count];
    let mut outcomes = vec![AdmissionOutcomeData::Unknown; result.original_input_count];
    for failure in &local_failures {
        if let Some(seen) = seen.get_mut(failure.index) {
            *seen = true;
            outcomes[failure.index] = AdmissionOutcomeData::Rejected;
        }
    }
    for item in &result.items {
        let Some(slot) = seen.get_mut(item.index) else {
            continue;
        };
        if *slot {
            continue;
        }
        *slot = true;
        outcomes[item.index] = match item.outcome {
            ProductListingIngestionOutcome::Accepted => AdmissionOutcomeData::Published,
            ProductListingIngestionOutcome::Unconfirmed => AdmissionOutcomeData::Unknown,
            ProductListingIngestionOutcome::NotAttempted { .. } => {
                AdmissionOutcomeData::NotAttempted
            }
            ProductListingIngestionOutcome::Rejected { .. } => AdmissionOutcomeData::Rejected,
        };
        if let Some(failure) = outcome_failure(item, source_ids.get(item.index).cloned().flatten())
        {
            local_failures.push(failure);
        } else {
            accepted += 1;
        }
    }
    // Inconsistent use-case results must never be reported as confirmed acceptance.
    for (index, present) in seen.iter().enumerate() {
        if !present {
            local_failures.push(BatchFailure {
                index,
                source_listing_id: source_ids.get(index).cloned().flatten(),
                error: "ENQUEUE_UNCONFIRMED",
                retryable: true,
                kind: FailureKind::Send,
            });
        }
    }
    local_failures.sort_by_key(|failure| failure.index);
    let items = outcomes
        .into_iter()
        .enumerate()
        .map(|(index, outcome)| {
            let failure = local_failures.iter().find(|failure| failure.index == index);
            AdmissionItemData {
                index,
                source_listing_id: source_ids.get(index).cloned().flatten(),
                outcome,
                error: failure.map(|failure| failure.error),
                retryable: failure.is_some_and(|failure| failure.retryable),
            }
        })
        .collect();
    AdmissionReportData {
        report: BatchReport {
            submission_id: result.submission_id.to_string(),
            accepted_count: accepted,
            failures: local_failures,
        },
        idempotency_key: result.idempotency_key.as_str().to_owned(),
        original_input_count: result.original_input_count,
        items,
    }
}

fn outcome_failure(
    item: &ProductListingIngestionItemOutcome,
    source_listing_id: Option<String>,
) -> Option<BatchFailure> {
    use ProductListingIngestionOutcome as Outcome;
    let (error, retryable, kind) = match &item.outcome {
        Outcome::Accepted => return None,
        Outcome::Unconfirmed => ("ENQUEUE_UNCONFIRMED", true, FailureKind::Send),
        Outcome::NotAttempted { reason, retryable } => (
            match reason {
                ProductListingIngestionNotAttemptedReason::DeadlineExceeded => {
                    "ENQUEUE_NOT_ATTEMPTED"
                }
                ProductListingIngestionNotAttemptedReason::BlockedByFifoPredecessor => {
                    "ENQUEUE_BLOCKED"
                }
            },
            *retryable,
            if *retryable {
                FailureKind::Send
            } else {
                FailureKind::Internal
            },
        ),
        Outcome::Rejected { reason, retryable } => match reason {
            ProductListingIngestionRejectionReason::Publisher { code } => match code.as_str() {
                "INVALID_MESSAGE_SIZE" => (
                    "PRODUCT_LISTING_INGESTION_PAYLOAD_TOO_LARGE",
                    false,
                    FailureKind::MessageSize,
                ),
                "SQS_RETRYABLE_FAILURE" => ("ENQUEUE_FAILED", true, FailureKind::Send),
                "SQS_SENDER_FAILURE" | "SQS_NOT_SENT" => {
                    ("ENQUEUE_FAILED", false, FailureKind::Internal)
                }
                _ => (
                    "PRODUCT_LISTING_INGESTION_INTERNAL_ERROR",
                    false,
                    FailureKind::Internal,
                ),
            },
            _ => ("BAD_BODY_VALUE", *retryable, FailureKind::Invalid),
        },
    };
    Some(BatchFailure {
        index: item.index,
        source_listing_id,
        error,
        retryable,
        kind,
    })
}

#[cfg(test)]
mod tests;
