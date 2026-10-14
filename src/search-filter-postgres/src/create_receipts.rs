use crate::mapping::{name, product_search_from_json, product_search_to_json, state};
use application::error::{box_error, static_error};
use application::idempotency::{CommandReceiptError, CommandReceipts, CommandReceiptsFactory};
use platform_postgres::{SqlxTransaction, idempotency::coordinate_command};
use search_filter_service::{
    create_receipt::CreateSearchFilterIntent, ports::SearchFilterView,
    use_cases::CreateSearchFilterResult,
};
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

#[derive(Debug, Clone, Default)]
pub struct SqlxSearchFilterCreateReceiptsFactory;

struct SqlxSearchFilterCreateReceipts<'tx> {
    tx: &'tx mut SqlxTransaction,
}

impl CommandReceiptsFactory<SqlxTransaction, CreateSearchFilterIntent, CreateSearchFilterResult>
    for SqlxSearchFilterCreateReceiptsFactory
{
    fn in_transaction<'tx>(
        &'tx self,
        tx: &'tx mut SqlxTransaction,
    ) -> impl CommandReceipts<CreateSearchFilterIntent, CreateSearchFilterResult> + 'tx {
        SqlxSearchFilterCreateReceipts { tx }
    }
}

// Explicit V1 application outcome codec. Never serialize HTTP responses or current aggregate rows.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct CreateResultV1 {
    user_search_filter_id: uuid::Uuid,
    user_id: uuid::Uuid,
    name: String,
    notifications: bool,
    state: String,
    search: serde_json::Value,
    embedding: Option<Vec<f32>>,
    #[serde(with = "time::serde::rfc3339")]
    created: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    updated: OffsetDateTime,
}

// V1 snapshots retain the same fixed vector shape as their original persisted creation.
const V1_EMBEDDING_DIMENSIONS: usize = 768;

fn invalid(source: impl std::error::Error + Send + Sync + 'static) -> CommandReceiptError {
    CommandReceiptError::InvalidPersistedState(box_error(source))
}
fn storage(source: impl std::error::Error + Send + Sync + 'static) -> CommandReceiptError {
    CommandReceiptError::Storage(box_error(source))
}

#[async_trait::async_trait]
impl CommandReceipts<CreateSearchFilterIntent, CreateSearchFilterResult>
    for SqlxSearchFilterCreateReceipts<'_>
{
    async fn find(
        &mut self,
        intent: &CreateSearchFilterIntent,
    ) -> Result<Option<CreateSearchFilterResult>, CommandReceiptError> {
        let row = sqlx::query_as::<_, (i32, Vec<u8>, i32, Option<serde_json::Value>, uuid::Uuid)>(
            "SELECT fingerprint_version,fingerprint,outcome_version,outcome,user_search_filter_id FROM search_filter_create_receipts WHERE user_id=$1 AND actor=$2 AND idempotency_key=$3"
        ).bind(intent.user_id.into_uuid()).bind(&intent.actor).bind(intent.key.as_str())
            .fetch_optional(self.tx.connection()).await.map_err(storage)?;
        let Some((fingerprint_version, fingerprint, outcome_version, outcome, id)) = row else {
            return Ok(None);
        };
        if fingerprint_version != 1 || fingerprint.len() != 32 || outcome_version != 1 {
            return Err(CommandReceiptError::InvalidPersistedState(static_error(
                "unsupported command receipt codec",
            )));
        }
        if fingerprint != intent.fingerprint.as_bytes() {
            return Err(CommandReceiptError::Mismatch);
        }
        let outcome = outcome.ok_or(CommandReceiptError::ResultUnavailable)?;
        let snapshot: CreateResultV1 = serde_json::from_value(outcome).map_err(invalid)?;
        if snapshot.user_id != intent.user_id.into_uuid()
            || snapshot.user_search_filter_id != id
            || snapshot.state != "ACTIVE"
            || snapshot.created != snapshot.updated
        {
            return Err(CommandReceiptError::InvalidPersistedState(static_error(
                "invalid command receipt identity or creation state",
            )));
        }
        if let Some(embedding) = &snapshot.embedding {
            // Snapshots validate their V1 shape independently of current aggregate state.
            if embedding.len() != V1_EMBEDDING_DIMENSIONS
                || embedding.iter().any(|v| !v.is_finite())
            {
                return Err(CommandReceiptError::InvalidPersistedState(static_error(
                    "invalid receipt embedding",
                )));
            }
        }
        let filter = SearchFilterView {
            search_filter_id:
                search_filter_core::user_search_filter_id::UserSearchFilterId::try_from(
                    snapshot.user_search_filter_id,
                )
                .map_err(invalid)?,
            user_id: user_core::user_id::UserId::try_from(snapshot.user_id).map_err(invalid)?,
            name: name(snapshot.name).map_err(invalid)?,
            notifications: snapshot.notifications,
            state: state(&snapshot.state).map_err(invalid)?,
            search: product_search_from_json(snapshot.search).map_err(invalid)?,
            embedding: snapshot.embedding,
            created: snapshot.created,
            updated: snapshot.updated,
        };
        let result = CreateSearchFilterResult { filter };
        if !intent.matches_result(&result) {
            return Err(CommandReceiptError::InvalidPersistedState(static_error(
                "command receipt outcome does not match its bound intent",
            )));
        }
        Ok(Some(result))
    }

    async fn coordinate(
        &mut self,
        intent: &CreateSearchFilterIntent,
    ) -> Result<(), CommandReceiptError> {
        let identity = serde_json::to_vec(&(
            "CREATE_SEARCH_FILTER",
            intent.user_id.to_string(),
            &intent.actor,
            intent.key.as_str(),
        ))
        .map_err(storage)?;
        coordinate_command(self.tx.connection(), &identity).await
    }

    async fn complete(
        &mut self,
        intent: &CreateSearchFilterIntent,
        outcome: &CreateSearchFilterResult,
    ) -> Result<(), CommandReceiptError> {
        let view = &outcome.filter;
        let snapshot = CreateResultV1 {
            user_search_filter_id: view.search_filter_id.into_uuid(),
            user_id: view.user_id.into_uuid(),
            name: view.name.to_string(),
            notifications: view.notifications,
            state: view.state.as_str().into(),
            search: product_search_to_json(&view.search).map_err(storage)?,
            embedding: view.embedding.clone(),
            created: view.created,
            updated: view.updated,
        };
        let outcome = serde_json::to_value(snapshot).map_err(storage)?;
        sqlx::query("INSERT INTO search_filter_create_receipts(user_id,actor,idempotency_key,fingerprint_version,fingerprint,user_search_filter_id,outcome_version,outcome) VALUES ($1,$2,$3,1,$4,$5,1,$6)")
            .bind(intent.user_id.into_uuid()).bind(&intent.actor).bind(intent.key.as_str()).bind(intent.fingerprint.as_bytes().as_slice())
            .bind(view.search_filter_id.into_uuid()).bind(outcome).execute(self.tx.connection()).await.map_err(storage)?;
        Ok(())
    }
}
