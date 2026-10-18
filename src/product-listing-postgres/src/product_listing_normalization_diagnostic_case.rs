//! PostgreSQL storage for the normalization diagnostic replay corpus.

use crate::object_id::try_from_uuid;
use application::error::box_error;
use product_listing_normalization::{
    NormalizationContext, ProductListingNormalizationInput, RawProductListingOperation,
    RawProductListingPayloadFormat, RawProductListingValues, SourcePayload,
};
use product_service::ports::{
    MAX_NORMALIZATION_DIAGNOSTIC_BUILD_BYTES, NormalizationDiagnosticCase,
    NormalizationDiagnosticCaseId, NormalizationDiagnosticCasePageRequest,
    NormalizationDiagnosticCasePortError, NormalizationDiagnosticCaseReader,
    NormalizationDiagnosticCaseRecorder, NormalizationDiagnosticCaseWriter,
    NormalizationDiagnosticCaseWriterFactory, NormalizationDiagnosticEntryPoint,
    RetainedProductListingRawRevision, RetainedProductListingRawRevisionRead,
    RetainedProductListingRawRevisionReader, StoredNormalizationDiagnosticCase,
    StoredNormalizationDiagnosticEvidence, UnreadableProductListingRawRevision,
};
use sqlx::{PgConnection, PgPool};

#[derive(Debug, Clone, Copy, Default)]
pub struct SqlxNormalizationDiagnosticCaseWriterFactory;

impl SqlxNormalizationDiagnosticCaseWriterFactory {
    pub fn new() -> Self {
        Self
    }
}

struct SqlxNormalizationDiagnosticCaseWriter<'tx> {
    connection: &'tx mut PgConnection,
}

/// Pool-backed recorder, corpus reader and retained raw-revision reader.
#[derive(Debug, Clone)]
pub struct SqlxNormalizationDiagnosticCaseStore {
    pool: PgPool,
}

impl SqlxNormalizationDiagnosticCaseStore {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

impl NormalizationDiagnosticCaseWriterFactory<platform_postgres::SqlxTransaction>
    for SqlxNormalizationDiagnosticCaseWriterFactory
{
    fn in_transaction<'tx>(
        &'tx self,
        tx: &'tx mut platform_postgres::SqlxTransaction,
    ) -> impl NormalizationDiagnosticCaseWriter + 'tx {
        SqlxNormalizationDiagnosticCaseWriter {
            connection: tx.connection(),
        }
    }
}

#[async_trait::async_trait]
impl NormalizationDiagnosticCaseWriter for SqlxNormalizationDiagnosticCaseWriter<'_> {
    async fn record(
        &mut self,
        case: NormalizationDiagnosticCase,
    ) -> Result<NormalizationDiagnosticCaseId, NormalizationDiagnosticCasePortError> {
        record_case(&mut *self.connection, case).await
    }
}

#[async_trait::async_trait]
impl NormalizationDiagnosticCaseRecorder for SqlxNormalizationDiagnosticCaseStore {
    async fn record(
        &self,
        case: NormalizationDiagnosticCase,
    ) -> Result<NormalizationDiagnosticCaseId, NormalizationDiagnosticCasePortError> {
        let mut connection = self.pool.acquire().await.map_err(persistence)?;
        record_case(&mut connection, case).await
    }
}

async fn record_case(
    connection: &mut PgConnection,
    case: NormalizationDiagnosticCase,
) -> Result<NormalizationDiagnosticCaseId, NormalizationDiagnosticCasePortError> {
    let case_id = case.case_id();
    let issues = serde_json::to_value(&case.issues).map_err(persistence)?;
    let build = bounded_build(&case.provenance.build);
    let diagnostics_version = i16::try_from(case.provenance.diagnostics_version)
        .map_err(|_| invalid_state("diagnostics version exceeds storage range"))?;
    let evaluator_version = i16::try_from(case.provenance.evaluator_version)
        .map_err(|_| invalid_state("evaluator version exceeds storage range"))?;
    // Evidence identity is immutable; later evaluations refresh outcome and provenance only.
    sqlx::query_scalar::<_, uuid::Uuid>(
        r#"
        INSERT INTO product_listing_normalization_diagnostic_cases (
            case_id, entry_point, input_sha256, listing_source_id,
            product_listing_raw_revision_id, outcome, issues,
            diagnostics_version, evaluator_version, build
        )
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)
        ON CONFLICT (entry_point, input_sha256) DO UPDATE SET
            outcome = EXCLUDED.outcome,
            issues = EXCLUDED.issues,
            diagnostics_version = EXCLUDED.diagnostics_version,
            evaluator_version = EXCLUDED.evaluator_version,
            build = EXCLUDED.build,
            occurrences = product_listing_normalization_diagnostic_cases.occurrences + 1,
            last_recorded_at = now()
        RETURNING case_id
        "#,
    )
    .bind(case_id.as_uuid())
    .bind(case.entry_point.as_str())
    .bind(case.input_sha256.as_slice())
    .bind(case.listing_source_id.as_uuid())
    .bind(case.product_listing_raw_revision_id.as_uuid())
    .bind(case.outcome.as_str())
    .bind(issues)
    .bind(diagnostics_version)
    .bind(evaluator_version)
    .bind(build)
    .fetch_one(connection)
    .await
    .map(NormalizationDiagnosticCaseId::from_uuid)
    .map_err(persistence)
}

fn bounded_build(build: &str) -> &str {
    let build = build.trim();
    if build.is_empty() {
        return "unknown";
    }
    let mut end = build.len().min(MAX_NORMALIZATION_DIAGNOSTIC_BUILD_BYTES);
    while !build.is_char_boundary(end) {
        end -= 1;
    }
    &build[..end]
}

#[derive(Debug, sqlx::FromRow)]
struct CaseRow {
    case_id: uuid::Uuid,
    generation: i64,
    entry_point: String,
    input_sha256: Vec<u8>,
    listing_source_id: uuid::Uuid,
    product_listing_raw_revision_id: uuid::Uuid,
    outcome: String,
    issues: serde_json::Value,
    diagnostics_version: i16,
    evaluator_version: i16,
    build: String,
    occurrences: i64,
    operation: Option<String>,
    payload_format: Option<String>,
    payload_schema_version: Option<i16>,
    raw_values_schema_version: Option<i16>,
    source_payload: Option<serde_json::Value>,
    raw_values: Option<serde_json::Value>,
    normalization_context: Option<serde_json::Value>,
}

#[async_trait::async_trait]
impl NormalizationDiagnosticCaseReader for SqlxNormalizationDiagnosticCaseStore {
    async fn snapshot_bound(&self) -> Result<Option<i64>, NormalizationDiagnosticCasePortError> {
        sqlx::query_scalar::<_, Option<i64>>(
            "SELECT max(generation) FROM product_listing_normalization_diagnostic_cases",
        )
        .fetch_one(&self.pool)
        .await
        .map_err(persistence)
    }

    async fn page(
        &self,
        request: NormalizationDiagnosticCasePageRequest,
    ) -> Result<Vec<StoredNormalizationDiagnosticCase>, NormalizationDiagnosticCasePortError> {
        let filter = request.filter;
        let issue_filter = (filter.field.is_some() || filter.reason.is_some()).then(|| {
            let mut issue = serde_json::Map::new();
            if let Some(field) = filter.field {
                issue.insert("field".to_owned(), serde_json::Value::String(field));
            }
            if let Some(reason) = filter.reason {
                issue.insert("reason".to_owned(), serde_json::Value::String(reason));
            }
            serde_json::Value::Array(vec![serde_json::Value::Object(issue)])
        });
        let case_ids = filter
            .case_ids
            .iter()
            .map(NormalizationDiagnosticCaseId::as_uuid)
            .collect::<Vec<_>>();
        let rows = sqlx::query_as::<_, CaseRow>(
            r#"
            SELECT
                c.case_id, c.generation, c.entry_point, c.input_sha256, c.listing_source_id,
                c.product_listing_raw_revision_id, c.outcome, c.issues,
                c.diagnostics_version, c.evaluator_version, c.build, c.occurrences,
                r.operation, r.payload_format, r.payload_schema_version,
                r.raw_values_schema_version, r.source_payload, r.raw_values,
                r.normalization_context
            FROM product_listing_normalization_diagnostic_cases AS c
            LEFT JOIN product_listing_raw_revisions AS r
              ON r.product_listing_raw_revision_id = c.product_listing_raw_revision_id
            WHERE c.generation > $1
              AND c.generation <= $2
              AND ($3::text IS NULL OR c.entry_point = $3)
              AND ($4::uuid IS NULL OR c.listing_source_id = $4)
              AND (cardinality($5::uuid[]) = 0 OR c.case_id = ANY($5))
              AND ($6::jsonb IS NULL OR c.issues @> $6)
            ORDER BY c.generation
            LIMIT $7
            "#,
        )
        .bind(request.after_generation)
        .bind(request.through_generation)
        .bind(
            filter
                .entry_point
                .map(NormalizationDiagnosticEntryPoint::as_str),
        )
        .bind(filter.listing_source_id.map(|id| *id.as_uuid()))
        .bind(case_ids)
        .bind(issue_filter)
        .bind(i64::from(request.limit))
        .fetch_all(&self.pool)
        .await
        .map_err(persistence)?;
        Ok(rows.into_iter().map(stored_case).collect())
    }
}

fn stored_case(row: CaseRow) -> StoredNormalizationDiagnosticCase {
    let entry_point = NormalizationDiagnosticEntryPoint::from_code(&row.entry_point);
    let evidence = match (
        try_from_uuid(
            row.product_listing_raw_revision_id,
            "ProductListing raw revision ID",
        ),
        stored_input(
            row.operation,
            row.payload_format,
            row.payload_schema_version,
            row.raw_values_schema_version,
            row.source_payload,
            row.raw_values,
            row.normalization_context,
        ),
    ) {
        (Ok(product_listing_raw_revision_id), Ok(input)) => {
            StoredNormalizationDiagnosticEvidence::RawRevision {
                product_listing_raw_revision_id,
                input: Box::new(input),
            }
        }
        (Err(_), _) => StoredNormalizationDiagnosticEvidence::Unavailable {
            reason: "RAW_REVISION_ID_INVALID",
        },
        (_, Err(reason)) => StoredNormalizationDiagnosticEvidence::Unavailable { reason },
    };
    StoredNormalizationDiagnosticCase {
        case_id: NormalizationDiagnosticCaseId::from_uuid(row.case_id),
        generation: row.generation,
        entry_point,
        entry_point_code: row.entry_point,
        input_sha256: row.input_sha256,
        listing_source_id: row.listing_source_id,
        evidence,
        recorded_outcome: row.outcome,
        recorded_issues: row.issues,
        diagnostics_version: row.diagnostics_version,
        evaluator_version: row.evaluator_version,
        build: row.build,
        occurrences: row.occurrences,
    }
}

/// Rebuilds a stored input through the validating constructors. Failures are reported as
/// unavailable evidence for that case rather than aborting the replay.
#[allow(clippy::too_many_arguments)]
fn stored_input(
    operation: Option<String>,
    payload_format: Option<String>,
    payload_schema_version: Option<i16>,
    raw_values_schema_version: Option<i16>,
    source_payload: Option<serde_json::Value>,
    raw_values: Option<serde_json::Value>,
    normalization_context: Option<serde_json::Value>,
) -> Result<ProductListingNormalizationInput, &'static str> {
    let (
        Some(operation),
        Some(payload_format),
        Some(payload_schema_version),
        Some(raw_values_schema_version),
        Some(source_payload),
        Some(raw_values),
        Some(normalization_context),
    ) = (
        operation,
        payload_format,
        payload_schema_version,
        raw_values_schema_version,
        source_payload,
        raw_values,
        normalization_context,
    )
    else {
        return Err("RAW_REVISION_MISSING");
    };
    ProductListingNormalizationInput::new(
        RawProductListingOperation::from_code(&operation).ok_or("RAW_REVISION_INVALID")?,
        RawProductListingPayloadFormat::from_code(&payload_format).ok_or("RAW_REVISION_INVALID")?,
        u16::try_from(payload_schema_version).map_err(|_| "RAW_REVISION_INVALID")?,
        u16::try_from(raw_values_schema_version).map_err(|_| "RAW_REVISION_INVALID")?,
        SourcePayload::new(source_payload).map_err(|_| "RAW_REVISION_INVALID")?,
        RawProductListingValues::new(raw_values).map_err(|_| "RAW_REVISION_INVALID")?,
        NormalizationContext::new(normalization_context).map_err(|_| "RAW_REVISION_INVALID")?,
    )
    .map_err(|_| "RAW_REVISION_INVALID")
}

#[derive(Debug, sqlx::FromRow)]
struct RetainedRevisionRow {
    generation: i64,
    product_listing_raw_revision_id: uuid::Uuid,
    listing_source_id: uuid::Uuid,
    operation: String,
    payload_format: String,
    payload_schema_version: i16,
    raw_values_schema_version: i16,
    source_payload: serde_json::Value,
    raw_values: serde_json::Value,
    normalization_context: serde_json::Value,
}

#[async_trait::async_trait]
impl RetainedProductListingRawRevisionReader for SqlxNormalizationDiagnosticCaseStore {
    async fn snapshot_bound(&self) -> Result<Option<i64>, NormalizationDiagnosticCasePortError> {
        sqlx::query_scalar::<_, Option<i64>>(
            "SELECT max(generation) FROM product_listing_raw_revisions",
        )
        .fetch_one(&self.pool)
        .await
        .map_err(persistence)
    }

    async fn page(
        &self,
        after_generation: i64,
        through_generation: i64,
        limit: u32,
    ) -> Result<Vec<RetainedProductListingRawRevisionRead>, NormalizationDiagnosticCasePortError>
    {
        let rows = sqlx::query_as::<_, RetainedRevisionRow>(
            r#"
            SELECT
                r.generation, r.product_listing_raw_revision_id, s.listing_source_id,
                r.operation, r.payload_format, r.payload_schema_version,
                r.raw_values_schema_version, r.source_payload, r.raw_values,
                r.normalization_context
            FROM product_listing_raw_revisions AS r
            JOIN product_listing_raw_streams AS s
              ON s.product_listing_raw_stream_id = r.product_listing_raw_stream_id
            WHERE r.operation = 'UPSERT'
              AND r.generation > $1
              AND r.generation <= $2
            ORDER BY r.generation
            LIMIT $3
            "#,
        )
        .bind(after_generation)
        .bind(through_generation)
        .bind(i64::from(limit))
        .fetch_all(&self.pool)
        .await
        .map_err(persistence)?;
        Ok(rows
            .into_iter()
            .map(|row| {
                let unreadable = || {
                    RetainedProductListingRawRevisionRead::Unreadable(
                        UnreadableProductListingRawRevision {
                            generation: row.generation,
                            product_listing_raw_revision_id: row.product_listing_raw_revision_id,
                        },
                    )
                };
                let revision_id = try_from_uuid(
                    row.product_listing_raw_revision_id,
                    "ProductListing raw revision ID",
                );
                let listing_source_id = try_from_uuid(row.listing_source_id, "ListingSource ID");
                let input = stored_input(
                    Some(row.operation.clone()),
                    Some(row.payload_format.clone()),
                    Some(row.payload_schema_version),
                    Some(row.raw_values_schema_version),
                    Some(row.source_payload.clone()),
                    Some(row.raw_values.clone()),
                    Some(row.normalization_context.clone()),
                );
                match (revision_id, listing_source_id, input) {
                    (Ok(product_listing_raw_revision_id), Ok(listing_source_id), Ok(input)) => {
                        RetainedProductListingRawRevisionRead::Readable(Box::new(
                            RetainedProductListingRawRevision {
                                generation: row.generation,
                                product_listing_raw_revision_id,
                                listing_source_id,
                                input,
                            },
                        ))
                    }
                    _ => unreadable(),
                }
            })
            .collect())
    }
}

fn persistence(
    error: impl std::error::Error + Send + Sync + 'static,
) -> NormalizationDiagnosticCasePortError {
    NormalizationDiagnosticCasePortError::Persistence {
        source: box_error(error),
    }
}

fn invalid_state(message: &'static str) -> NormalizationDiagnosticCasePortError {
    NormalizationDiagnosticCasePortError::InvalidPersistedState {
        source: box_error(std::io::Error::other(message)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_provenance_is_bounded_and_never_blank() {
        assert_eq!("unknown", bounded_build("  "));
        assert_eq!("abc", bounded_build(" abc "));
        assert_eq!(
            MAX_NORMALIZATION_DIAGNOSTIC_BUILD_BYTES,
            bounded_build(&"é".repeat(64)).len()
        );
    }
}
