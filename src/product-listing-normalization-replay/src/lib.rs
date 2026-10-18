//! `normalization-replay`: reruns the retained raw-revision normalization corpus against the
//! current checkout.
//!
//! Replay calls the production raw-values evaluator on complete saved inputs and writes
//! machine-readable results. It never writes canonical state, advances raw heads, redelivers
//! queue messages or records replay outcomes as new cases. See `docs/normalization-replay.md`.

use product_listing_normalization::shipping::ShippingQuickCheck;
use product_listing_normalization::{
    ListingAvailabilityQuickCheck, NORMALIZATION_DIAGNOSTICS_VERSION, NormalizationIssue,
    NormalizationIssueHandling, NormalizationIssueScope,
    PRODUCT_LISTING_RAW_VALUES_NORMALIZER_VERSION, ProductListingNormalizationInput,
    ProductListingRawValuesNormalizationOutcome, ProductListingRawValuesNormalizer,
    ProductListingRawValuesPatch, ProductListingRawValuesResolved,
};
use product_service::normalization_diagnostics::BUILD_GIT_SHA;
use product_service::ports::{
    NormalizationDiagnosticCase, NormalizationDiagnosticCaseFilter,
    NormalizationDiagnosticCasePageRequest, NormalizationDiagnosticCaseReader,
    NormalizationDiagnosticCaseRecorder, NormalizationDiagnosticEntryPoint,
    NormalizationDiagnosticOutcome, NormalizationDiagnosticProvenance,
    RetainedProductListingRawRevisionRead, RetainedProductListingRawRevisionReader,
    StoredNormalizationDiagnosticCase, StoredNormalizationDiagnosticEvidence,
};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::io::Write;

pub const DEFAULT_REPLAY_PAGE_SIZE: u32 = 200;

/// Portable replay case: the JSONL export/import format and the unit replay operates on.
#[derive(Debug, Clone, PartialEq)]
pub struct ReplayCase {
    pub case_id: String,
    pub entry_point: String,
    pub input_sha256: String,
    pub listing_source_id: String,
    pub recorded_outcome: String,
    pub recorded_issues: Value,
    pub evidence: ReplayEvidence,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ReplayEvidence {
    /// Full normalization input document of an immutable raw revision.
    RawRevision {
        product_listing_raw_revision_id: String,
        input: Value,
    },
    Unavailable {
        reason: String,
    },
}

impl ReplayCase {
    pub fn from_stored(case: StoredNormalizationDiagnosticCase) -> Self {
        let evidence = match case.evidence {
            StoredNormalizationDiagnosticEvidence::RawRevision {
                product_listing_raw_revision_id,
                input,
            } => ReplayEvidence::RawRevision {
                product_listing_raw_revision_id: product_listing_raw_revision_id.to_string(),
                input: input.to_document(),
            },
            StoredNormalizationDiagnosticEvidence::Unavailable { reason } => {
                ReplayEvidence::Unavailable {
                    reason: reason.to_owned(),
                }
            }
        };
        Self {
            case_id: case.case_id.to_string(),
            entry_point: case.entry_point_code,
            input_sha256: hex(&case.input_sha256),
            listing_source_id: case.listing_source_id.to_string(),
            recorded_outcome: case.recorded_outcome,
            recorded_issues: case.recorded_issues,
            evidence,
        }
    }

    pub fn to_json(&self) -> Value {
        let evidence = match &self.evidence {
            ReplayEvidence::RawRevision {
                product_listing_raw_revision_id,
                input,
            } => json!({
                "kind": "RAW_REVISION",
                "productListingRawRevisionId": product_listing_raw_revision_id,
                "input": input,
            }),
            ReplayEvidence::Unavailable { reason } => {
                json!({"kind": "UNAVAILABLE", "reason": reason})
            }
        };
        json!({
            "caseId": self.case_id,
            "entryPoint": self.entry_point,
            "inputSha256": self.input_sha256,
            "listingSourceId": self.listing_source_id,
            "recordedOutcome": self.recorded_outcome,
            "recordedIssues": self.recorded_issues,
            "evidence": evidence,
        })
    }

    pub fn from_json(value: &Value) -> Result<Self, String> {
        let text = |name: &str| {
            value
                .get(name)
                .and_then(Value::as_str)
                .map(str::to_owned)
                .ok_or_else(|| format!("replay case field `{name}` is missing or not a string"))
        };
        let evidence = value
            .get("evidence")
            .ok_or_else(|| "replay case field `evidence` is missing".to_owned())?;
        let evidence = match evidence.get("kind").and_then(Value::as_str) {
            Some("RAW_REVISION") => ReplayEvidence::RawRevision {
                product_listing_raw_revision_id: evidence
                    .get("productListingRawRevisionId")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned(),
                input: evidence.get("input").cloned().unwrap_or(Value::Null),
            },
            Some("UNAVAILABLE") => ReplayEvidence::Unavailable {
                reason: evidence
                    .get("reason")
                    .and_then(Value::as_str)
                    .unwrap_or("UNKNOWN")
                    .to_owned(),
            },
            _ => return Err("replay case evidence kind is unsupported".to_owned()),
        };
        Ok(Self {
            case_id: text("caseId")?,
            entry_point: text("entryPoint")?,
            input_sha256: text("inputSha256")?,
            listing_source_id: text("listingSourceId")?,
            recorded_outcome: text("recordedOutcome")?,
            recorded_issues: value.get("recordedIssues").cloned().unwrap_or(json!([])),
            evidence,
        })
    }
}

/// Current classification of one replayed case or recorded issue.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ReplayStatus {
    Resolved,
    Unresolved,
    Invalid,
    Incompatible,
    SystemError,
    UnavailableEvidence,
}

impl ReplayStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Resolved => "RESOLVED",
            Self::Unresolved => "UNRESOLVED",
            Self::Invalid => "INVALID",
            Self::Incompatible => "INCOMPATIBLE",
            Self::SystemError => "SYSTEM_ERROR",
            Self::UnavailableEvidence => "UNAVAILABLE_EVIDENCE",
        }
    }
}

/// Result of replaying one case with the current production evaluator.
#[derive(Debug, Clone, PartialEq)]
pub struct ReplayResult {
    pub status: ReplayStatus,
    pub decision: Option<&'static str>,
    pub fields: Value,
    pub issues: Vec<NormalizationIssue>,
    /// Current status of each recorded `(field, reason)` pair.
    pub recorded_issue_statuses: Vec<(String, String, ReplayStatus)>,
    pub error: Option<String>,
}

/// Calls the current production raw-values evaluator on the case's complete input.
pub fn replay_case(case: &ReplayCase) -> ReplayResult {
    let entry_point = NormalizationDiagnosticEntryPoint::from_code(&case.entry_point);
    let evaluated = match (&case.evidence, entry_point) {
        (ReplayEvidence::Unavailable { reason }, _) => {
            Err((ReplayStatus::UnavailableEvidence, reason.clone()))
        }
        (
            ReplayEvidence::RawRevision { input, .. },
            Some(NormalizationDiagnosticEntryPoint::RawValuesNormalizationV1),
        ) => ProductListingNormalizationInput::from_document(input)
            .map(|input| evaluate_raw_values(&input))
            .map_err(|error| (ReplayStatus::Incompatible, error.to_string())),
        _ => Err((
            ReplayStatus::Incompatible,
            "evidence does not match the entry point".to_owned(),
        )),
    };
    let recorded = recorded_issue_keys(&case.recorded_issues);
    match evaluated {
        Ok((decision, fields, issues)) => {
            let rejected = decision == "INVALID";
            let status = if issues
                .iter()
                .any(|issue| issue.scope == NormalizationIssueScope::System)
            {
                ReplayStatus::SystemError
            } else if rejected {
                ReplayStatus::Invalid
            } else if issues.is_empty() {
                ReplayStatus::Resolved
            } else {
                ReplayStatus::Unresolved
            };
            let recorded_issue_statuses = recorded
                .into_iter()
                .map(|(field, reason)| {
                    let current = issues
                        .iter()
                        .filter(|issue| issue.field.as_str() == field)
                        .collect::<Vec<_>>();
                    let field_status = if current.is_empty() {
                        ReplayStatus::Resolved
                    } else if current
                        .iter()
                        .any(|issue| issue.scope == NormalizationIssueScope::System)
                    {
                        ReplayStatus::SystemError
                    } else if current
                        .iter()
                        .any(|issue| issue.handling == NormalizationIssueHandling::RejectedInput)
                    {
                        ReplayStatus::Invalid
                    } else {
                        ReplayStatus::Unresolved
                    };
                    (field, reason, field_status)
                })
                .collect();
            ReplayResult {
                status,
                decision: Some(decision),
                fields,
                issues,
                recorded_issue_statuses,
                error: None,
            }
        }
        Err((status, error)) => ReplayResult {
            status,
            decision: None,
            fields: Value::Null,
            issues: Vec::new(),
            recorded_issue_statuses: recorded
                .into_iter()
                .map(|(field, reason)| (field, reason, status))
                .collect(),
            error: Some(error),
        },
    }
}

fn evaluate_raw_values(
    input: &ProductListingNormalizationInput,
) -> (&'static str, Value, Vec<NormalizationIssue>) {
    let evaluation = ProductListingRawValuesNormalizer::new().evaluate(input);
    let issues = evaluation.report.into_issues();
    match evaluation.outcome {
        ProductListingRawValuesNormalizationOutcome::Resolved(resolved) => {
            ("RESOLVED", resolved_fields(&resolved), issues)
        }
        ProductListingRawValuesNormalizationOutcome::Invalid(error) => (
            "INVALID",
            json!({"error": error.to_string(), "scope": format!("{:?}", error.failure_scope())}),
            issues,
        ),
        ProductListingRawValuesNormalizationOutcome::Delete => ("DELETE", Value::Null, issues),
    }
}

fn recorded_issue_keys(recorded: &Value) -> Vec<(String, String)> {
    let mut keys = recorded
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|issue| {
            Some((
                issue.get("field")?.as_str()?.to_owned(),
                issue.get("reason")?.as_str()?.to_owned(),
            ))
        })
        .collect::<Vec<_>>();
    keys.sort();
    keys.dedup();
    keys
}

fn patch<T>(patch: &ProductListingRawValuesPatch<T>, value: impl Fn(&T) -> Value) -> Value {
    match patch {
        ProductListingRawValuesPatch::Set(inner) => json!({"action": "SET", "value": value(inner)}),
        ProductListingRawValuesPatch::Clear => json!({"action": "CLEAR"}),
        ProductListingRawValuesPatch::Unchanged => json!({"action": "UNCHANGED"}),
    }
}

fn price(price: &money::Price) -> Value {
    json!({"amountMinor": *price.monetary_amount, "currency": price.currency.as_str()})
}

fn listing_price(
    value: &product_listing_core::product_listing_price::ProductListingPrice,
) -> Value {
    match value {
        product_listing_core::product_listing_price::ProductListingPrice::Monetary(value) => {
            json!({"kind": "MONETARY", "price": price(value)})
        }
        product_listing_core::product_listing_price::ProductListingPrice::OnRequest => {
            json!({"kind": "ON_REQUEST"})
        }
    }
}

fn availability(value: &ListingAvailabilityQuickCheck) -> Value {
    match value {
        ListingAvailabilityQuickCheck::Resolved(value) => json!(value.as_str()),
        ListingAvailabilityQuickCheck::NoAssertion => json!("NO_ASSERTION"),
        ListingAvailabilityQuickCheck::Unsupported(_) => json!("UNSUPPORTED"),
    }
}

fn shipping(value: &ShippingQuickCheck) -> Value {
    match value {
        ShippingQuickCheck::Resolved(assertions) => json!({
            "kind": "RESOLVED",
            "prohibited": assertions.prohibited,
            "worldwide": assertions.worldwide,
            "offered": assertions.offered,
            "price": assertions.price.as_ref().map(|price| format!("{price:?}")),
            "handling": assertions.handling.as_ref().map(|range| json!({
                "minimum": range.minimum(),
                "maximum": range.maximum(),
            })),
        }),
        ShippingQuickCheck::NoAssertion => json!({"kind": "NO_ASSERTION"}),
        ShippingQuickCheck::Unsupported(_) => json!({"kind": "UNSUPPORTED"}),
    }
}

fn resolved_fields(resolved: &ProductListingRawValuesResolved) -> Value {
    json!({
        "sourceListingId": resolved.source_listing_id.to_string(),
        "title": patch(&resolved.title, |title| json!({
            "language": title.localization.as_str(),
            "value": title.payload.as_ref(),
        })),
        "description": patch(&resolved.description, |description| json!({
            "language": description.localization.as_str(),
        })),
        "price": patch(&resolved.price, listing_price),
        "priceEstimateMin": patch(&resolved.price_estimate_min, price),
        "priceEstimateMax": patch(&resolved.price_estimate_max, price),
        "availability": patch(&resolved.availability, availability),
        "url": patch(&resolved.url, |url| json!(url.as_str())),
        "images": patch(&resolved.images, |images| json!(images.len())),
        "shipping": shipping(&resolved.shipping),
    })
}

/// Counts by case status and by recorded `(field, reason)` and its current status.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ReplaySummary {
    pub cases: u64,
    pub statuses: BTreeMap<&'static str, u64>,
    pub recorded_issues: BTreeMap<String, BTreeMap<&'static str, u64>>,
}

impl ReplaySummary {
    pub fn add(&mut self, result: &ReplayResult) {
        self.cases += 1;
        *self.statuses.entry(result.status.as_str()).or_default() += 1;
        for (field, reason, status) in &result.recorded_issue_statuses {
            *self
                .recorded_issues
                .entry(format!("{field}/{reason}"))
                .or_default()
                .entry(status.as_str())
                .or_default() += 1;
        }
    }

    pub fn to_json(&self, completed: bool, error: Option<&str>) -> Value {
        json!({
            "type": "summary",
            "completed": completed,
            "error": error,
            "cases": self.cases,
            "statuses": self.statuses,
            "recordedIssues": self.recorded_issues,
            "diagnosticsVersion": NORMALIZATION_DIAGNOSTICS_VERSION,
            "rawValuesEvaluatorVersion": PRODUCT_LISTING_RAW_VALUES_NORMALIZER_VERSION,
            "gitSha": BUILD_GIT_SHA,
        })
    }
}

pub fn result_record(case: &ReplayCase, result: &ReplayResult) -> Value {
    json!({
        "type": "case",
        "caseId": case.case_id,
        "entryPoint": case.entry_point,
        "inputSha256": case.input_sha256,
        "listingSourceId": case.listing_source_id,
        "recordedOutcome": case.recorded_outcome,
        "recordedIssues": case.recorded_issues,
        "status": result.status.as_str(),
        "decision": result.decision,
        "fields": result.fields,
        "issues": result.issues,
        "error": result.error,
    })
}

/// How a run ended. An incomplete run stopped before covering its whole snapshot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunCompletion {
    pub summary: ReplaySummary,
    pub error: Option<String>,
}

impl RunCompletion {
    pub fn completed(&self) -> bool {
        self.error.is_none()
    }
}

/// Replays every case of the corpus snapshot bounded at run start, page by page.
pub async fn replay_corpus(
    reader: &dyn NormalizationDiagnosticCaseReader,
    filter: NormalizationDiagnosticCaseFilter,
    page_size: u32,
    out: &mut dyn Write,
) -> RunCompletion {
    let mut summary = ReplaySummary::default();
    let error = for_each_stored_case(reader, filter, page_size, |case| {
        let case = ReplayCase::from_stored(case);
        let result = replay_case(&case);
        summary.add(&result);
        write_record(out, &result_record(&case, &result))
    })
    .await
    .err();
    RunCompletion { summary, error }
}

/// Writes the selected corpus with full evidence for offline replay.
pub async fn export_corpus(
    reader: &dyn NormalizationDiagnosticCaseReader,
    filter: NormalizationDiagnosticCaseFilter,
    page_size: u32,
    out: &mut dyn Write,
) -> RunCompletion {
    let mut summary = ReplaySummary::default();
    let error = for_each_stored_case(reader, filter, page_size, |case| {
        summary.cases += 1;
        write_record(out, &ReplayCase::from_stored(case).to_json())
    })
    .await
    .err();
    RunCompletion { summary, error }
}

/// Replays exported JSONL cases without database access. Malformed lines are reported as
/// incompatible cases and the run continues.
pub fn replay_jsonl(input: impl std::io::BufRead, out: &mut dyn Write) -> RunCompletion {
    let mut summary = ReplaySummary::default();
    for (index, line) in input.lines().enumerate() {
        let line = match line {
            Ok(line) => line,
            Err(error) => {
                return RunCompletion {
                    summary,
                    error: Some(format!("failed to read input line {}: {error}", index + 1)),
                };
            }
        };
        if line.trim().is_empty() {
            continue;
        }
        let case = serde_json::from_str::<Value>(&line)
            .map_err(|error| error.to_string())
            .and_then(|value| ReplayCase::from_json(&value));
        let (case, result) = match case {
            Ok(case) => {
                let result = replay_case(&case);
                (case, result)
            }
            Err(error) => {
                let case = ReplayCase {
                    case_id: format!("line-{}", index + 1),
                    entry_point: String::new(),
                    input_sha256: String::new(),
                    listing_source_id: String::new(),
                    recorded_outcome: String::new(),
                    recorded_issues: json!([]),
                    evidence: ReplayEvidence::Unavailable {
                        reason: "MALFORMED_CASE".to_owned(),
                    },
                };
                let result = ReplayResult {
                    status: ReplayStatus::Incompatible,
                    decision: None,
                    fields: Value::Null,
                    issues: Vec::new(),
                    recorded_issue_statuses: Vec::new(),
                    error: Some(error),
                };
                (case, result)
            }
        };
        summary.add(&result);
        if let Err(error) = write_record(out, &result_record(&case, &result)) {
            return RunCompletion {
                summary,
                error: Some(error),
            };
        }
    }
    RunCompletion {
        summary,
        error: None,
    }
}

async fn for_each_stored_case(
    reader: &dyn NormalizationDiagnosticCaseReader,
    filter: NormalizationDiagnosticCaseFilter,
    page_size: u32,
    mut visit: impl FnMut(StoredNormalizationDiagnosticCase) -> Result<(), String>,
) -> Result<(), String> {
    let page_size = page_size.max(1);
    let Some(through_generation) = reader
        .snapshot_bound()
        .await
        .map_err(|error| format!("failed to read corpus snapshot bound: {error}"))?
    else {
        return Ok(());
    };
    let mut after_generation = 0;
    loop {
        let page = reader
            .page(NormalizationDiagnosticCasePageRequest {
                after_generation,
                through_generation,
                limit: page_size,
                filter: filter.clone(),
            })
            .await
            .map_err(|error| {
                format!("failed to read corpus page after generation {after_generation}: {error}")
            })?;
        let page_len = page.len();
        for case in page {
            after_generation = case.generation;
            visit(case)?;
        }
        if page_len < page_size as usize {
            return Ok(());
        }
    }
}

/// Historical seeding: evaluates every retained UPSERT raw revision with the production
/// evaluator and records the ones with issues as `EVALUATED_*` cases.
pub async fn seed_from_raw_revisions(
    revisions: &dyn RetainedProductListingRawRevisionReader,
    recorder: &dyn NormalizationDiagnosticCaseRecorder,
    page_size: u32,
) -> SeedCompletion {
    let mut completion = SeedCompletion::default();
    let page_size = page_size.max(1);
    let through_generation = match revisions.snapshot_bound().await {
        Ok(Some(bound)) => bound,
        Ok(None) => return completion,
        Err(error) => {
            completion.error = Some(format!("failed to read raw revision bound: {error}"));
            return completion;
        }
    };
    let mut after_generation = 0;
    loop {
        let page = match revisions
            .page(after_generation, through_generation, page_size)
            .await
        {
            Ok(page) => page,
            Err(error) => {
                completion.error = Some(format!(
                    "failed to read raw revisions after generation {after_generation}: {error}"
                ));
                return completion;
            }
        };
        let page_len = page.len();
        for revision in page {
            completion.evaluated += 1;
            let revision = match revision {
                RetainedProductListingRawRevisionRead::Readable(revision) => revision,
                RetainedProductListingRawRevisionRead::Unreadable(revision) => {
                    after_generation = revision.generation;
                    completion.unavailable += 1;
                    continue;
                }
            };
            after_generation = revision.generation;
            let evaluation = ProductListingRawValuesNormalizer::new().evaluate(&revision.input);
            if evaluation.report.is_empty() {
                continue;
            }
            let Ok(input_sha256) = revision.input.hash() else {
                completion.unavailable += 1;
                continue;
            };
            let outcome = match evaluation.outcome {
                ProductListingRawValuesNormalizationOutcome::Invalid(_) => {
                    NormalizationDiagnosticOutcome::EvaluatedInvalid
                }
                _ => NormalizationDiagnosticOutcome::EvaluatedResolved,
            };
            let case = NormalizationDiagnosticCase {
                entry_point: NormalizationDiagnosticEntryPoint::RawValuesNormalizationV1,
                input_sha256: *input_sha256.as_bytes(),
                listing_source_id: revision.listing_source_id,
                product_listing_raw_revision_id: revision.product_listing_raw_revision_id,
                outcome,
                issues: evaluation.report.into_issues(),
                provenance: NormalizationDiagnosticProvenance {
                    diagnostics_version: NORMALIZATION_DIAGNOSTICS_VERSION,
                    evaluator_version: PRODUCT_LISTING_RAW_VALUES_NORMALIZER_VERSION,
                    build: BUILD_GIT_SHA.to_owned(),
                },
            };
            if let Err(error) = recorder.record(case).await {
                completion.error = Some(format!(
                    "failed to record seeded case for raw revision {}: {error}",
                    revision.product_listing_raw_revision_id
                ));
                return completion;
            }
            completion.recorded += 1;
        }
        if page_len < page_size as usize {
            return completion;
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SeedCompletion {
    pub evaluated: u64,
    pub recorded: u64,
    pub unavailable: u64,
    pub error: Option<String>,
}

impl SeedCompletion {
    pub fn to_json(&self) -> Value {
        json!({
            "type": "seedSummary",
            "completed": self.error.is_none(),
            "error": self.error,
            "evaluatedRevisions": self.evaluated,
            "recordedCases": self.recorded,
            "unavailableEvidence": self.unavailable,
        })
    }
}

pub fn write_record(out: &mut dyn Write, record: &Value) -> Result<(), String> {
    serde_json::to_writer(&mut *out, record)
        .map_err(|error| error.to_string())
        .and_then(|()| out.write_all(b"\n").map_err(|error| error.to_string()))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use listing_source_core::ListingSourceId;
    use product_listing_core::product_listing_raw_id::ProductListingRawRevisionId;
    use product_service::ports::{
        NormalizationDiagnosticCaseId, NormalizationDiagnosticCasePortError,
        RetainedProductListingRawRevision,
    };
    use std::sync::Mutex;

    const REVIEWED_CASES: &str = include_str!("../tests/fixtures/reviewed_cases.jsonl");

    fn issue_keys(issues: &[NormalizationIssue]) -> Vec<String> {
        issues
            .iter()
            .map(|issue| format!("{}/{}", issue.field.as_str(), issue.reason.as_str()))
            .collect()
    }

    /// Reviewed expected outputs pin repaired cases, previously successful inputs and
    /// negative or ambiguous examples. "Now resolves" alone is never accepted as correct.
    #[test]
    fn reviewed_fixtures_match_current_production_evaluators() {
        let mut categories = std::collections::BTreeSet::new();
        for line in REVIEWED_CASES
            .lines()
            .filter(|line| !line.trim().is_empty())
        {
            let fixture: Value = serde_json::from_str(line).unwrap();
            let name = fixture["name"].as_str().unwrap();
            categories.insert(fixture["category"].as_str().unwrap().to_owned());
            let case = ReplayCase::from_json(&fixture["case"]).unwrap();
            let result = replay_case(&case);
            let expected = &fixture["expected"];
            assert_eq!(
                expected["status"].as_str(),
                Some(result.status.as_str()),
                "{name}: {result:?}"
            );
            assert_eq!(expected["decision"].as_str(), result.decision, "{name}");
            assert_eq!(
                expected["issues"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|issue| issue.as_str().unwrap().to_owned())
                    .collect::<Vec<_>>(),
                issue_keys(&result.issues),
                "{name}"
            );
            if let Some(fields) = expected["fields"].as_object() {
                for (field, value) in fields {
                    assert_eq!(value, &result.fields[field], "{name}: {field}");
                }
            }
        }
        assert_eq!(
            ["NEGATIVE", "REPAIRED", "SUCCESSFUL"]
                .into_iter()
                .map(str::to_owned)
                .collect::<std::collections::BTreeSet<_>>(),
            categories
        );
    }

    fn fixture_case(name: &str) -> ReplayCase {
        REVIEWED_CASES
            .lines()
            .map(|line| serde_json::from_str::<Value>(line).unwrap())
            .find(|fixture| fixture["name"] == name)
            .map(|fixture| ReplayCase::from_json(&fixture["case"]).unwrap())
            .unwrap()
    }

    #[test]
    fn replay_matches_direct_production_calls() {
        let case = fixture_case("missing-currency-preserves-price");
        let ReplayEvidence::RawRevision { input, .. } = &case.evidence else {
            panic!("raw revision fixture");
        };
        let input = ProductListingNormalizationInput::from_document(input).unwrap();
        let direct = ProductListingRawValuesNormalizer::new().evaluate(&input);
        assert_eq!(direct.report.issues(), replay_case(&case).issues.as_slice());
    }

    #[test]
    fn recorded_issues_report_their_current_status() {
        let mut case = fixture_case("european-price-and-quantity");
        case.recorded_issues = json!([
            {"field": "PRICE", "reason": "PRICE_EXCESS_PRECISION"},
        ]);
        let result = replay_case(&case);
        assert_eq!(
            vec![(
                "PRICE".to_owned(),
                "PRICE_EXCESS_PRECISION".to_owned(),
                ReplayStatus::Resolved
            )],
            result.recorded_issue_statuses
        );

        case.evidence = ReplayEvidence::Unavailable {
            reason: "RAW_REVISION_MISSING".to_owned(),
        };
        assert_eq!(ReplayStatus::UnavailableEvidence, replay_case(&case).status);

        case.entry_point = "UNKNOWN_ENTRY_POINT_V9".to_owned();
        case.evidence = ReplayEvidence::RawRevision {
            product_listing_raw_revision_id: String::new(),
            input: json!({}),
        };
        assert_eq!(ReplayStatus::Incompatible, replay_case(&case).status);
    }

    struct CorpusFake {
        cases: Vec<StoredNormalizationDiagnosticCase>,
        bound: i64,
        fail_after: Option<i64>,
        requests: Mutex<Vec<NormalizationDiagnosticCasePageRequest>>,
    }

    fn stored(generation: i64) -> StoredNormalizationDiagnosticCase {
        let case = fixture_case("negated-quantity");
        let ReplayEvidence::RawRevision { input, .. } = case.evidence else {
            panic!("raw revision fixture");
        };
        StoredNormalizationDiagnosticCase {
            case_id: NormalizationDiagnosticCaseId::from_uuid(uuid::Uuid::from_u128(
                generation as u128,
            )),
            generation,
            entry_point: Some(NormalizationDiagnosticEntryPoint::RawValuesNormalizationV1),
            entry_point_code: "RAW_VALUES_NORMALIZATION_V1".to_owned(),
            input_sha256: vec![0; 32],
            listing_source_id: uuid::Uuid::nil(),
            evidence: StoredNormalizationDiagnosticEvidence::RawRevision {
                product_listing_raw_revision_id: ProductListingRawRevisionId::new(),
                input: Box::new(ProductListingNormalizationInput::from_document(&input).unwrap()),
            },
            recorded_outcome: "APPLIED".to_owned(),
            recorded_issues: json!([{"field": "AVAILABILITY", "reason": "AVAILABILITY_QUALIFIED_QUANTITY"}]),
            diagnostics_version: 1,
            evaluator_version: 7,
            build: "test".to_owned(),
            occurrences: 1,
        }
    }

    #[async_trait]
    impl NormalizationDiagnosticCaseReader for CorpusFake {
        async fn snapshot_bound(
            &self,
        ) -> Result<Option<i64>, NormalizationDiagnosticCasePortError> {
            Ok(Some(self.bound))
        }

        async fn page(
            &self,
            request: NormalizationDiagnosticCasePageRequest,
        ) -> Result<Vec<StoredNormalizationDiagnosticCase>, NormalizationDiagnosticCasePortError>
        {
            self.requests.lock().unwrap().push(request.clone());
            if self
                .fail_after
                .is_some_and(|generation| request.after_generation >= generation)
            {
                return Err(NormalizationDiagnosticCasePortError::Persistence {
                    source: application::error::box_error(std::io::Error::other("down")),
                });
            }
            Ok(self
                .cases
                .iter()
                .filter(|case| {
                    case.generation > request.after_generation
                        && case.generation <= request.through_generation
                })
                .take(request.limit as usize)
                .cloned()
                .collect())
        }
    }

    fn records(output: &[u8]) -> Vec<Value> {
        String::from_utf8_lossy(output)
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect()
    }

    #[tokio::test]
    async fn unfiltered_replay_pages_through_the_snapshot_bounded_at_run_start() {
        // Generation 6 was recorded after the run started and belongs to the next run.
        let corpus = CorpusFake {
            cases: (1..=6).map(stored).collect(),
            bound: 5,
            fail_after: None,
            requests: Mutex::new(Vec::new()),
        };
        let mut output = Vec::new();
        let completion = replay_corpus(
            &corpus,
            NormalizationDiagnosticCaseFilter::default(),
            2,
            &mut output,
        )
        .await;
        assert!(completion.completed());
        assert_eq!(5, completion.summary.cases);
        assert_eq!(Some(&5), completion.summary.statuses.get("UNRESOLVED"));
        assert_eq!(
            Some(&5),
            completion.summary.recorded_issues["AVAILABILITY/AVAILABILITY_QUALIFIED_QUANTITY"]
                .get("UNRESOLVED")
        );
        assert_eq!(5, records(&output).len());
        assert!(
            corpus
                .requests
                .lock()
                .unwrap()
                .iter()
                .all(|request| request.through_generation == 5 && request.limit == 2)
        );
    }

    #[tokio::test]
    async fn interrupted_replay_is_reported_incomplete() {
        let corpus = CorpusFake {
            cases: (1..=4).map(stored).collect(),
            bound: 4,
            fail_after: Some(2),
            requests: Mutex::new(Vec::new()),
        };
        let mut output = Vec::new();
        let completion = replay_corpus(
            &corpus,
            NormalizationDiagnosticCaseFilter::default(),
            2,
            &mut output,
        )
        .await;
        assert!(!completion.completed());
        assert_eq!(2, completion.summary.cases);
        let summary = completion
            .summary
            .to_json(completion.completed(), completion.error.as_deref());
        assert_eq!(json!(false), summary["completed"]);
    }

    #[tokio::test]
    async fn exported_cases_replay_offline_and_malformed_lines_are_reported() {
        let corpus = CorpusFake {
            cases: vec![stored(1)],
            bound: 1,
            fail_after: None,
            requests: Mutex::new(Vec::new()),
        };
        let mut export = Vec::new();
        assert!(
            export_corpus(
                &corpus,
                NormalizationDiagnosticCaseFilter::default(),
                10,
                &mut export
            )
            .await
            .completed()
        );
        export.extend_from_slice(b"{\"not\": \"a case\"}\n");

        let mut output = Vec::new();
        let completion = replay_jsonl(export.as_slice(), &mut output);
        assert!(completion.completed());
        let records = records(&output);
        assert_eq!(json!("UNRESOLVED"), records[0]["status"]);
        assert_eq!(json!("INCOMPATIBLE"), records[1]["status"]);
    }

    struct RevisionsFake(Vec<RetainedProductListingRawRevisionRead>);

    #[async_trait]
    impl RetainedProductListingRawRevisionReader for RevisionsFake {
        async fn snapshot_bound(
            &self,
        ) -> Result<Option<i64>, NormalizationDiagnosticCasePortError> {
            Ok(Some(self.0.len() as i64))
        }

        async fn page(
            &self,
            after_generation: i64,
            through_generation: i64,
            limit: u32,
        ) -> Result<Vec<RetainedProductListingRawRevisionRead>, NormalizationDiagnosticCasePortError>
        {
            Ok(self
                .0
                .iter()
                .enumerate()
                .filter(|(index, _)| {
                    let generation = *index as i64 + 1;
                    generation > after_generation && generation <= through_generation
                })
                .take(limit as usize)
                .map(|(_, revision)| revision.clone())
                .collect())
        }
    }

    #[derive(Default)]
    struct RecorderFake(Mutex<Vec<NormalizationDiagnosticCase>>);

    #[async_trait]
    impl NormalizationDiagnosticCaseRecorder for RecorderFake {
        async fn record(
            &self,
            case: NormalizationDiagnosticCase,
        ) -> Result<NormalizationDiagnosticCaseId, NormalizationDiagnosticCasePortError> {
            let case_id = case.case_id();
            self.0.lock().unwrap().push(case);
            Ok(case_id)
        }
    }

    #[tokio::test]
    async fn seeding_records_retained_revisions_with_issues_using_the_same_evaluator() {
        let revision = |generation: i64, name: &str| {
            let ReplayEvidence::RawRevision { input, .. } = fixture_case(name).evidence else {
                panic!("raw revision fixture");
            };
            RetainedProductListingRawRevisionRead::Readable(Box::new(
                RetainedProductListingRawRevision {
                    generation,
                    product_listing_raw_revision_id: ProductListingRawRevisionId::new(),
                    listing_source_id: ListingSourceId::new(),
                    input: ProductListingNormalizationInput::from_document(&input).unwrap(),
                },
            ))
        };
        let revisions = RevisionsFake(vec![
            revision(1, "european-price-and-quantity"),
            revision(2, "negative-display-price"),
            revision(3, "negated-quantity"),
        ]);
        let recorder = RecorderFake::default();
        let completion = seed_from_raw_revisions(&revisions, &recorder, 2).await;
        assert_eq!(
            SeedCompletion {
                evaluated: 3,
                recorded: 2,
                unavailable: 0,
                error: None,
            },
            completion
        );
        let cases = recorder.0.lock().unwrap();
        assert_eq!(
            vec![
                NormalizationDiagnosticOutcome::EvaluatedInvalid,
                NormalizationDiagnosticOutcome::EvaluatedResolved
            ],
            cases.iter().map(|case| case.outcome).collect::<Vec<_>>()
        );
    }
}
