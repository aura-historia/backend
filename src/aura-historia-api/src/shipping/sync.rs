use crate::{
    auth::{TokenAuthenticator, protected_context},
    error::*,
    wire::parse_path_object_id,
};
use axum::{
    Json,
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use listing_source_core::ListingSourceId;
use serde::Deserialize;
use serde_json::{Value, json};
use shipping_sync_json as codec;
use shipping_sync_service::{use_cases::*, *};
use std::sync::Arc;
use time::format_description::well_known::Rfc3339;
use uuid::Uuid;

#[derive(Clone)]
pub struct ShippingSyncState {
    pub sync: Arc<dyn ShippingSyncUseCase>,
    pub authenticator: Arc<dyn TokenAuthenticator>,
}
fn bad() -> ApiError {
    ApiError::bad_request(BAD_BODY_VALUE).with_detail("Invalid SHIP_SYNC_V1 input.")
}
#[derive(Deserialize)]
enum Version {
    #[serde(rename = "SHIP_SYNC_V1")]
    V1,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct InstallationData {
    schema: Version,
    integration: Value,
    expected_revision: i64,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CurrentData {
    schema: Version,
    integration: Value,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct BeginData {
    schema: Version,
    epoch: Uuid,
    key: String,
    scope: String,
    mode: String,
    expected_generation: i64,
    expected_sequence: i64,
    provider_version: String,
    components: [bool; 3],
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ManifestData {
    schema: Version,
    chunks: u32,
    records: u32,
    digest: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CleanupData {
    schema: Version,
    limit: u32,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PendingQuery {
    after: Option<Uuid>,
    size: Option<u32>,
}
fn parse<T: serde::de::DeserializeOwned>(body: &str) -> Result<T, ApiError> {
    serde_json::from_str(body).map_err(|_| bad())
}
fn id(value: &str) -> Result<Uuid, ApiError> {
    let id: Uuid = value.parse().map_err(|_| bad())?;
    if id.to_string() != value {
        return Err(bad());
    }
    Ok(id)
}
fn error(e: SyncError) -> ApiError {
    match e {
        SyncError::InvalidInput => bad(),
        SyncError::Conflict | SyncError::Dirty => ApiError::conflict(SHIPPING_SYNC_CONFLICT),
        SyncError::Revoked => ApiError::conflict(SHIPPING_SYNC_REVOKED),
        SyncError::NotFound => ApiError::not_found(SHIPPING_SYNC_NOT_FOUND),
        SyncError::Incomplete => ApiError::conflict(SHIPPING_SYNC_INCOMPLETE),
        SyncError::Limit => ApiError::conflict(SHIPPING_SYNC_LIMIT),
        SyncError::Authorization(e) => e.into(),
        SyncError::Listing(_)
        | SyncError::Storage { .. }
        | SyncError::Transaction(_)
        | SyncError::Dependency(_) => {
            ApiError::service_unavailable(SHIPPING_TEMPORARILY_UNAVAILABLE)
        }
    }
}
fn response(result: Result<SyncResult, SyncError>) -> Response {
    let result = match result {
        Ok(r) => r,
        Err(e) => return error(e).into_response(),
    };
    let (status, value) = match result {
        SyncResult::Cleaned { sessions } => (
            StatusCode::OK,
            json!({"schema":"SHIP_SYNC_V1","erasedSessions":sessions}),
        ),
        SyncResult::Binding(b) => (
            StatusCode::OK,
            json!({"schema":"SHIP_SYNC_V1","epoch":b.epoch,"revision":b.revision,"active":b.active,"integration":shipping_json::listing_delivery::encode_integration(&b.integration).ok()}),
        ),
        SyncResult::Head(h) => (
            StatusCode::OK,
            json!({"schema":"SHIP_SYNC_V1","generation":h.generation,"sequence":h.sequence,"reconciliationRequired":h.dirty}),
        ),
        SyncResult::Pending(ids) => (
            StatusCode::OK,
            json!({"schema":"SHIP_SYNC_V1","sessionIds":ids}),
        ),
        SyncResult::Receipt(r) => {
            let status = match r.state {
                ApplicationState::Applied => StatusCode::OK,
                ApplicationState::WaitingDependency => StatusCode::FAILED_DEPENDENCY,
                ApplicationState::ReconcileRequired | ApplicationState::Expired => {
                    StatusCode::CONFLICT
                }
                _ => StatusCode::ACCEPTED,
            };
            (
                status,
                json!({"schema":"SHIP_SYNC_V1","sessionId":r.id,"epoch":r.begin.epoch,"scope":r.begin.scope.as_str(),"custody":"ACCEPTED","reconciliationRequired":r.reconciliation_required,"rawCapture":if r.raw_complete{"COMPLETE"}else{"IN_PROGRESS"},"capturedChunks":r.captured_chunks,"capturedRecords":r.captured_records,"chunks":r.chunk_digests.iter().map(|(i,d)|json!({"index":i,"digest":codec::hex(d)})).collect::<Vec<_>>(),"application":r.state.as_str(),"applied":r.state==ApplicationState::Applied,"generation":r.generation,"failure":r.failure,"projection":"UNKNOWN","startedAt":r.started_at.format(&Rfc3339).ok(),"expiresAt":r.expires_at.format(&Rfc3339).ok(),"manifest":r.manifest.map(|m|json!({"schema":"SHIP_SYNC_V1","chunks":m.chunks,"records":m.records,"digest":codec::hex(&m.digest)}))}),
            )
        }
    };
    (status, Json(value)).into_response()
}
pub async fn cleanup(
    State(s): State<ShippingSyncState>,
    Path(source): Path<String>,
    headers: HeaderMap,
    body: String,
) -> Response {
    let command = (|| {
        let d: CleanupData = parse(&body)?;
        let _ = d.schema;
        Ok(SyncCommand::Cleanup { limit: d.limit })
    })();
    execute(s, source, headers, command).await
}
async fn execute(
    s: ShippingSyncState,
    source: String,
    headers: HeaderMap,
    command: Result<SyncCommand, ApiError>,
) -> Response {
    let (context, _) = match protected_context(s.authenticator.as_ref(), &headers).await {
        Ok(v) => v,
        Err(e) => return *e,
    };
    let source = match parse_path_object_id::<ListingSourceId>(
        &source,
        "listing_source_id",
        "ListingSource",
    ) {
        Ok(v) => v,
        Err(e) => return e.into_response(),
    };
    match command {
        Ok(c) => response(s.sync.execute(&context, source, c).await),
        Err(e) => e.into_response(),
    }
}
pub async fn rotate(
    State(s): State<ShippingSyncState>,
    Path(source): Path<String>,
    headers: HeaderMap,
    body: String,
) -> Response {
    let command = (|| {
        let d: InstallationData = parse(&body)?;
        let _ = d.schema;
        Ok(SyncCommand::Rotate {
            integration: shipping_json::listing_delivery::decode_integration(d.integration)
                .map_err(|_| bad())?,
            expected_revision: d.expected_revision,
        })
    })();
    execute(s, source, headers, command).await
}
pub async fn current(
    State(s): State<ShippingSyncState>,
    Path(source): Path<String>,
    headers: HeaderMap,
    body: String,
) -> Response {
    let command = (|| {
        let d: CurrentData = parse(&body)?;
        let _ = d.schema;
        Ok(SyncCommand::Current {
            integration: shipping_json::listing_delivery::decode_integration(d.integration)
                .map_err(|_| bad())?,
        })
    })();
    execute(s, source, headers, command).await
}
pub async fn revoke(
    State(s): State<ShippingSyncState>,
    Path((source, epoch)): Path<(String, String)>,
    headers: HeaderMap,
) -> Response {
    execute(
        s,
        source,
        headers,
        id(&epoch).map(|epoch| SyncCommand::Revoke { epoch }),
    )
    .await
}
pub async fn begin(
    State(s): State<ShippingSyncState>,
    Path(source): Path<String>,
    headers: HeaderMap,
    body: String,
) -> Response {
    let command = (|| {
        let d: BeginData = parse(&body)?;
        let _ = d.schema;
        Ok(SyncCommand::Begin(Begin {
            epoch: d.epoch,
            key: ExternalKey::new(d.key).map_err(|_| bad())?,
            scope: ExternalKey::new(d.scope).map_err(|_| bad())?,
            mode: match d.mode.as_str() {
                "SNAPSHOT" => Mode::Snapshot,
                "DELTA" => Mode::Delta,
                _ => return Err(bad()),
            },
            expected_generation: d.expected_generation,
            expected_sequence: d.expected_sequence,
            provider_version: ExternalKey::new(d.provider_version).map_err(|_| bad())?,
            components: d.components,
        }))
    })();
    execute(s, source, headers, command).await
}
pub async fn head(
    State(s): State<ShippingSyncState>,
    Path((source, epoch, scope)): Path<(String, String, String)>,
    headers: HeaderMap,
) -> Response {
    let command = (|| {
        Ok(SyncCommand::Head {
            epoch: id(&epoch)?,
            scope: ExternalKey::new(scope).map_err(|_| bad())?,
        })
    })();
    execute(s, source, headers, command).await
}
pub async fn capture(
    State(s): State<ShippingSyncState>,
    Path((source, session, index)): Path<(String, String, u32)>,
    headers: HeaderMap,
    body: String,
) -> Response {
    let command = (|| {
        let source: ListingSourceId =
            parse_path_object_id(&source, "listing_source_id", "ListingSource")?;
        Ok(SyncCommand::Capture {
            id: id(&session)?,
            index,
            chunk: codec::parse_chunk(body.as_bytes(), source).map_err(|_| bad())?,
        })
    })();
    execute(s, source, headers, command).await
}
pub async fn finalize(
    State(s): State<ShippingSyncState>,
    Path((source, session)): Path<(String, String)>,
    headers: HeaderMap,
    body: String,
) -> Response {
    let command = (|| {
        let d: ManifestData = parse(&body)?;
        let _ = d.schema;
        Ok(SyncCommand::Finalize {
            id: id(&session)?,
            manifest: Manifest {
                chunks: d.chunks,
                records: d.records,
                digest: codec::digest(&d.digest).map_err(|_| bad())?,
            },
        })
    })();
    execute(s, source, headers, command).await
}
pub async fn status(
    State(s): State<ShippingSyncState>,
    Path((source, session)): Path<(String, String)>,
    headers: HeaderMap,
) -> Response {
    execute(
        s,
        source,
        headers,
        id(&session).map(|id| SyncCommand::Status { id }),
    )
    .await
}
pub async fn pending(
    State(s): State<ShippingSyncState>,
    Path((source, epoch)): Path<(String, String)>,
    headers: HeaderMap,
    query: Result<Query<PendingQuery>, axum::extract::rejection::QueryRejection>,
) -> Response {
    let command = (|| {
        let Query(q) = query.map_err(|_| bad())?;
        Ok(SyncCommand::Pending {
            epoch: id(&epoch)?,
            after: q.after,
            limit: q.size.unwrap_or(16),
        })
    })();
    execute(s, source, headers, command).await
}
