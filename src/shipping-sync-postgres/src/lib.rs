//! PostgreSQL custody, epoch fences and protected assertion ownership.
use application::error::{BoxError, box_error};
use listing_source_core::{ListingIngestionMethod, ListingSourceId};
use party_core::{party_id::PartyId, party_location_id::PartyLocationId};
use platform_postgres::SqlxTransaction;
use product_listing_core::{
    delivery::{DeliveryIntegration, ListingDelivery},
    source_listing_id::SourceListingId,
};
use shipping_core::profile::ShippingProfileId;
use shipping_json::{
    ShippingTermsReferenceCodec,
    listing_delivery::{self, DeliveryReferenceCodec},
};
use shipping_sync_json as codec;
use shipping_sync_service::{ports::*, *};
use sqlx::{FromRow, PgConnection};
use time::OffsetDateTime;
use uuid::Uuid;

#[derive(Clone, Copy, Default)]
pub struct SqlxShippingSyncStoreFactory;
struct Repository<'a>(&'a mut PgConnection);
impl SyncStoreFactory<SqlxTransaction> for SqlxShippingSyncStoreFactory {
    fn in_transaction<'a>(&'a self, tx: &'a mut SqlxTransaction) -> impl SyncStore + 'a {
        Repository(tx.connection())
    }
}
struct StoredReferences;
fn stored_uuid(s: &str) -> Result<Uuid, BoxError> {
    let id: Uuid = s.parse()?;
    if id.to_string() != s {
        return Err(box_error(std::io::Error::other(
            "invalid stored sync identity",
        )));
    }
    Ok(id)
}
impl ShippingTermsReferenceCodec for StoredReferences {
    fn decode_party(v: &str) -> Result<PartyId, BoxError> {
        PartyId::try_from(stored_uuid(v)?).map_err(box_error)
    }
    fn encode_party(v: PartyId) -> String {
        v.into_uuid().to_string()
    }
    fn decode_location(v: &str) -> Result<PartyLocationId, BoxError> {
        PartyLocationId::try_from(stored_uuid(v)?).map_err(box_error)
    }
    fn encode_location(v: PartyLocationId) -> String {
        v.into_uuid().to_string()
    }
}
impl DeliveryReferenceCodec for StoredReferences {
    fn decode_source(v: &str) -> Result<ListingSourceId, BoxError> {
        ListingSourceId::try_from(stored_uuid(v)?).map_err(box_error)
    }
    fn encode_source(v: ListingSourceId) -> String {
        v.into_uuid().to_string()
    }
    fn decode_profile(v: &str) -> Result<ShippingProfileId, BoxError> {
        ShippingProfileId::try_from(stored_uuid(v)?).map_err(box_error)
    }
    fn encode_profile(v: ShippingProfileId) -> String {
        v.into_uuid().to_string()
    }
}
fn encoding(source: BoxError) -> SyncError {
    SyncError::Storage { source }
}
fn corrupt() -> SyncError {
    failure(std::io::Error::other(
        "invalid shipping sync persisted state",
    ))
}
fn bytes(v: Vec<u8>) -> Result<[u8; 32], SyncError> {
    v.try_into().map_err(|_| corrupt())
}
fn integration_key(v: &DeliveryIntegration) -> String {
    match v {
        DeliveryIntegration::Source => "SOURCE".into(),
        DeliveryIntegration::Provider { method, domain } => {
            format!("{}:{}", method.as_str(), domain.as_str())
        }
    }
}

#[derive(FromRow)]
struct SessionRow {
    session_id: Uuid,
    listing_source_id: Uuid,
    epoch: Uuid,
    scope: String,
    fetch_key: String,
    mode: String,
    expected_generation: i64,
    expected_sequence: i64,
    provider_version: String,
    components: Vec<bool>,
    state: String,
    raw_complete: bool,
    manifest_chunks: Option<i32>,
    manifest_records: Option<i32>,
    manifest_digest: Option<Vec<u8>>,
    generation: Option<i64>,
    failure: Option<String>,
    started_at: OffsetDateTime,
    expires_at: OffsetDateTime,
}
impl SessionRow {
    fn receipt(self, chunks: i64, records: i64) -> Result<Receipt, SyncError> {
        let manifest = match (
            self.manifest_chunks,
            self.manifest_records,
            self.manifest_digest,
        ) {
            (None, None, None) => None,
            (Some(c), Some(r), Some(d))
                if c > 0 && c <= MAX_CHUNKS as i32 && r >= 0 && r <= MAX_RECORDS as i32 =>
            {
                Some(Manifest {
                    chunks: c as u32,
                    records: r as u32,
                    digest: bytes(d)?,
                })
            }
            _ => return Err(corrupt()),
        };
        let state = match self.state.as_str() {
            "OPEN" => ApplicationState::Open,
            "READY" => ApplicationState::Ready,
            "WAITING_DEPENDENCY" => ApplicationState::WaitingDependency,
            "APPLIED" => ApplicationState::Applied,
            "RECONCILE_REQUIRED" => ApplicationState::ReconcileRequired,
            "EXPIRED" => ApplicationState::Expired,
            _ => return Err(corrupt()),
        };
        if chunks < 0
            || chunks > i64::from(MAX_CHUNKS)
            || records < 0
            || records > i64::from(MAX_RECORDS)
            || (state == ApplicationState::Applied) != self.generation.is_some()
        {
            return Err(corrupt());
        }
        Ok(Receipt {
            id: self.session_id,
            source: ListingSourceId::try_from(self.listing_source_id).map_err(failure)?,
            begin: Begin {
                epoch: self.epoch,
                key: ExternalKey::new(self.fetch_key)?,
                scope: ExternalKey::new(self.scope)?,
                mode: match self.mode.as_str() {
                    "SNAPSHOT" => Mode::Snapshot,
                    "DELTA" => Mode::Delta,
                    _ => return Err(corrupt()),
                },
                expected_generation: self.expected_generation,
                expected_sequence: self.expected_sequence,
                provider_version: ExternalKey::new(self.provider_version)?,
                components: self.components.try_into().map_err(|_| corrupt())?,
            },
            state,
            raw_complete: self.raw_complete,
            captured_chunks: chunks as u32,
            captured_records: records as u32,
            chunk_digests: vec![],
            reconciliation_required: false,
            generation: self.generation,
            failure: self.failure,
            manifest,
            started_at: self.started_at,
            expires_at: self.expires_at,
        })
    }
}
#[derive(FromRow)]
struct ClaimRow {
    object_kind: String,
    external_key: String,
    scope: String,
    observed_at: OffsetDateTime,
    fresh_until: OffsetDateTime,
    expires_at: OffsetDateTime,
    applied: serde_json::Value,
    tombstoned: bool,
}
impl ClaimRow {
    fn claim(self, binding: &Binding) -> Result<Claim, SyncError> {
        let value = codec::decode_applied::<StoredReferences>(self.applied).map_err(encoding)?;
        let kind = match self.object_kind.as_str() {
            "PROFILE" => ObjectKind::Profile,
            "DEFAULT" => ObjectKind::Default,
            "ITEM" => ObjectKind::Item,
            _ => return Err(corrupt()),
        };
        if !matches!(
            (&value, kind),
            (AppliedValue::Profile { .. }, ObjectKind::Profile)
                | (AppliedValue::Default { .. }, ObjectKind::Default)
                | (AppliedValue::Item { .. }, ObjectKind::Item)
                | (AppliedValue::Tombstone, _)
        ) {
            return Err(corrupt());
        }
        if let AppliedValue::Item { delivery, .. } = &value
            && (delivery.source_id() != binding.source
                || delivery.integration() != &binding.integration)
        {
            return Err(corrupt());
        }
        if self.observed_at > self.fresh_until || self.fresh_until > self.expires_at {
            return Err(corrupt());
        }
        Ok(Claim {
            kind,
            key: ExternalKey::new(self.external_key)?,
            scope: ExternalKey::new(self.scope)?,
            window: ObservationWindow {
                observed_at: self.observed_at,
                fresh_until: self.fresh_until,
                expires_at: self.expires_at,
            },
            value,
            tombstoned: self.tombstoned,
        })
    }
}
impl Repository<'_> {
    async fn integration(
        &mut self,
        source: ListingSourceId,
        integration: &DeliveryIntegration,
    ) -> Result<(), SyncError> {
        if let DeliveryIntegration::Provider { method, domain } = integration {
            let enabled:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM listing_source_ingestion_methods WHERE listing_source_id=$1 AND ingestion_method=$2)").bind(source.into_uuid()).bind(method.as_str()).fetch_one(&mut *self.0).await.map_err(failure)?;
            let configured:Option<String>=match method {
                ListingIngestionMethod::Shopify=>sqlx::query_scalar("SELECT domain FROM listing_source_shopify_ingestion_configurations WHERE listing_source_id=$1 FOR SHARE").bind(source.into_uuid()).fetch_optional(&mut *self.0).await.map_err(failure)?,
                ListingIngestionMethod::Woocommerce=>sqlx::query_scalar::<_,Option<String>>("SELECT shipping_integration_domain FROM listing_source_woocommerce_ingestion_configurations WHERE listing_source_id=$1 FOR SHARE").bind(source.into_uuid()).fetch_optional(&mut *self.0).await.map_err(failure)?.flatten(),
                _=>return Err(SyncError::InvalidInput),
            };
            if !enabled || configured.as_deref() != Some(domain.as_str()) {
                return Err(SyncError::Revoked);
            }
        }
        Ok(())
    }
}
#[async_trait::async_trait]
impl SyncStore for Repository<'_> {
    async fn binding(
        &mut self,
        source: ListingSourceId,
        epoch: Uuid,
    ) -> Result<Binding, SyncError> {
        let row:Option<(i64,serde_json::Value,bool,Option<Uuid>)>=sqlx::query_as("SELECT i.revision,e.integration,e.active,e.current_source_id FROM shipping_sync_epochs e JOIN shipping_sync_installations i ON i.epoch=e.epoch AND i.listing_source_id=e.listing_source_id WHERE e.epoch=$1 AND e.listing_source_id=$2 FOR UPDATE OF e,i").bind(epoch).bind(source.into_uuid()).fetch_optional(&mut *self.0).await.map_err(failure)?;
        let (revision, integration, active, current) = row.ok_or(SyncError::Revoked)?;
        if !active || current != Some(source.into_uuid()) {
            return Err(SyncError::Revoked);
        }
        let integration = listing_delivery::decode_integration(integration).map_err(encoding)?;
        self.integration(source, &integration).await?;
        Ok(Binding {
            source,
            epoch,
            revision,
            integration,
            active,
        })
    }
    async fn rotate(
        &mut self,
        source: ListingSourceId,
        integration: &DeliveryIntegration,
        expected: i64,
    ) -> Result<Binding, SyncError> {
        self.integration(source, integration).await?;
        let key = integration_key(integration);
        let old:Option<(Uuid,i64)>=sqlx::query_as("SELECT epoch,revision FROM shipping_sync_installations WHERE listing_source_id=$1 AND integration_key=$2 FOR UPDATE").bind(source.into_uuid()).bind(&key).fetch_optional(&mut *self.0).await.map_err(failure)?;
        if old.map(|r| r.1).unwrap_or(0) != expected {
            return Err(SyncError::Conflict);
        }
        let revision = expected.checked_add(1).ok_or(SyncError::Conflict)?;
        if let Some((epoch, _)) = old {
            sqlx::query("UPDATE shipping_sync_epochs SET active=false WHERE epoch=$1")
                .bind(epoch)
                .execute(&mut *self.0)
                .await
                .map_err(failure)?;
        }
        let epoch = Uuid::now_v7();
        sqlx::query("INSERT INTO shipping_sync_epochs(epoch,listing_source_id,current_source_id,integration_key,integration,active) VALUES($1,$2,$2,$3,$4,true)").bind(epoch).bind(source.into_uuid()).bind(&key).bind(listing_delivery::encode_integration(integration).map_err(encoding)?).execute(&mut *self.0).await.map_err(failure)?;
        sqlx::query("INSERT INTO shipping_sync_installations(listing_source_id,integration_key,epoch,revision) VALUES($1,$2,$3,$4) ON CONFLICT(listing_source_id,integration_key) DO UPDATE SET epoch=$3,revision=$4").bind(source.into_uuid()).bind(key).bind(epoch).bind(revision).execute(&mut *self.0).await.map_err(failure)?;
        Ok(Binding {
            source,
            epoch,
            revision,
            integration: integration.clone(),
            active: true,
        })
    }
    async fn current(
        &mut self,
        source: ListingSourceId,
        integration: &DeliveryIntegration,
    ) -> Result<Binding, SyncError> {
        let row:Option<(Uuid,i64,bool)>=sqlx::query_as("SELECT i.epoch,i.revision,e.active FROM shipping_sync_installations i JOIN shipping_sync_epochs e USING(epoch) WHERE i.listing_source_id=$1 AND i.integration_key=$2 FOR SHARE OF i,e").bind(source.into_uuid()).bind(integration_key(integration)).fetch_optional(&mut *self.0).await.map_err(failure)?;
        let (epoch, revision, active) = row.ok_or(SyncError::NotFound)?;
        Ok(Binding {
            source,
            epoch,
            revision,
            integration: integration.clone(),
            active,
        })
    }
    async fn revoke(&mut self, binding: &Binding) -> Result<(), SyncError> {
        sqlx::query("UPDATE shipping_sync_epochs SET active=false WHERE epoch=$1")
            .bind(binding.epoch)
            .execute(&mut *self.0)
            .await
            .map_err(failure)?;
        sqlx::query("UPDATE shipping_sync_installations SET revision=revision+1 WHERE epoch=$1")
            .bind(binding.epoch)
            .execute(&mut *self.0)
            .await
            .map_err(failure)?;
        Ok(())
    }
    async fn cleanup(&mut self, source: ListingSourceId, limit: u32) -> Result<u32, SyncError> {
        let ids: Vec<Uuid> = sqlx::query_scalar("SELECT s.session_id FROM shipping_sync_sessions s WHERE s.listing_source_id=$1 AND s.started_at<clock_timestamp()-interval '30 days' AND EXISTS(SELECT 1 FROM shipping_sync_chunks c WHERE c.session_id=s.session_id AND c.payload IS NOT NULL) ORDER BY s.session_id LIMIT $2 FOR UPDATE SKIP LOCKED")
            .bind(source.into_uuid()).bind(i64::from(limit)).fetch_all(&mut *self.0).await.map_err(failure)?;
        sqlx::query("UPDATE shipping_sync_sessions SET state='EXPIRED',failure='SESSION_EXPIRED' WHERE session_id=ANY($1) AND state<>'APPLIED'").bind(&ids).execute(&mut *self.0).await.map_err(failure)?;
        sqlx::query("UPDATE shipping_sync_chunks SET payload=NULL,erased_at=clock_timestamp() WHERE session_id=ANY($1) AND payload IS NOT NULL").bind(&ids).execute(&mut *self.0).await.map_err(failure)?;
        Ok(ids.len() as u32)
    }
    async fn head(
        &mut self,
        binding: &Binding,
        scope: &ExternalKey,
    ) -> Result<ScopeHead, SyncError> {
        let row:Option<(i64,i64,bool)>=sqlx::query_as("SELECT COALESCE(s.generation,0),e.live_sequence,COALESCE((s.dirty OR (s.generation>0 AND s.configuration_sequence<>e.configuration_sequence)),false) FROM shipping_sync_epochs e LEFT JOIN shipping_sync_scopes s ON s.epoch=e.epoch AND s.scope=$2 WHERE e.epoch=$1 FOR UPDATE OF e").bind(binding.epoch).bind(scope.as_str()).fetch_optional(&mut *self.0).await.map_err(failure)?;
        // A never-begun scope has no row and starts at the installation's current fence.
        let (generation, sequence, dirty) = if let Some(row) = row {
            row
        } else {
            return Err(SyncError::Revoked);
        };
        Ok(ScopeHead {
            generation,
            sequence,
            dirty,
        })
    }
    async fn begin(&mut self, binding: &Binding, request: &Begin) -> Result<Receipt, SyncError> {
        let existing: Option<Uuid> = sqlx::query_scalar(
            "SELECT session_id FROM shipping_sync_sessions WHERE epoch=$1 AND fetch_key=$2",
        )
        .bind(binding.epoch)
        .bind(request.key.as_str())
        .fetch_optional(&mut *self.0)
        .await
        .map_err(failure)?;
        if let Some(id) = existing {
            let receipt = self.receipt(binding.source, id).await?;
            if receipt.begin != *request {
                return Err(SyncError::Conflict);
            }
            return Ok(receipt);
        }
        let open:i64=sqlx::query_scalar("SELECT count(*) FROM shipping_sync_sessions WHERE epoch=$1 AND expires_at>clock_timestamp() AND state IN ('OPEN','READY','WAITING_DEPENDENCY')").bind(binding.epoch).fetch_one(&mut *self.0).await.map_err(failure)?;
        if open >= MAX_OPEN_SESSIONS {
            return Err(SyncError::Limit);
        }
        sqlx::query(
            "INSERT INTO shipping_sync_scopes(epoch,scope) VALUES($1,$2) ON CONFLICT DO NOTHING",
        )
        .bind(binding.epoch)
        .bind(request.scope.as_str())
        .execute(&mut *self.0)
        .await
        .map_err(failure)?;
        let head = self.head(binding, &request.scope).await?;
        if head.generation != request.expected_generation
            || head.sequence != request.expected_sequence
        {
            return Err(SyncError::Conflict);
        }
        if request.mode == Mode::Delta {
            sqlx::query("UPDATE shipping_sync_epochs SET live_sequence=live_sequence+1 WHERE epoch=$1 AND $2::text IS NOT NULL").bind(binding.epoch).bind(request.scope.as_str()).execute(&mut *self.0).await.map_err(failure)?;
        }
        let id = Uuid::now_v7();
        sqlx::query("INSERT INTO shipping_sync_sessions(session_id,listing_source_id,epoch,scope,fetch_key,mode,expected_generation,expected_sequence,provider_version,components,state) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,'OPEN')").bind(id).bind(binding.source.into_uuid()).bind(binding.epoch).bind(request.scope.as_str()).bind(request.key.as_str()).bind(request.mode.as_str()).bind(request.expected_generation).bind(request.expected_sequence).bind(request.provider_version.as_str()).bind(request.components.as_slice()).execute(&mut *self.0).await.map_err(failure)?;
        self.receipt(binding.source, id).await
    }
    async fn receipt(&mut self, source: ListingSourceId, id: Uuid) -> Result<Receipt, SyncError> {
        let row=sqlx::query_as::<_,SessionRow>("SELECT * FROM shipping_sync_sessions WHERE session_id=$1 AND listing_source_id=$2 FOR UPDATE").bind(id).bind(source.into_uuid()).fetch_optional(&mut *self.0).await.map_err(failure)?.ok_or(SyncError::NotFound)?;
        let (chunks,records):(i64,i64)=sqlx::query_as("SELECT count(*),COALESCE(sum(records),0)::bigint FROM shipping_sync_chunks WHERE session_id=$1").bind(id).fetch_one(&mut *self.0).await.map_err(failure)?;
        let mut receipt = row.receipt(chunks, records)?;
        let digests:Vec<(i32,Vec<u8>)>=sqlx::query_as("SELECT chunk_index,digest FROM shipping_sync_chunks WHERE session_id=$1 ORDER BY chunk_index").bind(id).fetch_all(&mut *self.0).await.map_err(failure)?;
        receipt.chunk_digests = digests
            .into_iter()
            .map(|(i, d)| Ok((i as u32, bytes(d)?)))
            .collect::<Result<_, SyncError>>()?;
        receipt.reconciliation_required=sqlx::query_scalar("SELECT s.dirty OR (s.generation>0 AND s.configuration_sequence<>e.configuration_sequence) FROM shipping_sync_scopes s JOIN shipping_sync_epochs e USING(epoch) WHERE s.epoch=$1 AND s.scope=$2").bind(receipt.begin.epoch).bind(receipt.begin.scope.as_str()).fetch_one(&mut *self.0).await.map_err(failure)?;
        Ok(receipt)
    }
    async fn capture(
        &mut self,
        receipt: &Receipt,
        index: u32,
        chunk: &Chunk,
    ) -> Result<(), SyncError> {
        let payload = codec::encode_chunk::<StoredReferences>(&chunk.records).map_err(encoding)?;
        let validated = codec::decode_chunk::<StoredReferences>(payload.clone(), receipt.source)
            .map_err(encoding)?;
        if validated.digest != chunk.digest {
            return Err(SyncError::Conflict);
        }
        let existing:Option<(Vec<u8>,Option<serde_json::Value>)>=sqlx::query_as("SELECT digest,payload FROM shipping_sync_chunks WHERE session_id=$1 AND chunk_index=$2").bind(receipt.id).bind(index as i32).fetch_optional(&mut *self.0).await.map_err(failure)?;
        if let Some((hash, old)) = existing {
            if hash != chunk.digest || old.is_some_and(|old| old != payload) {
                return Err(SyncError::Conflict);
            }
            return Ok(());
        }
        if receipt.state != ApplicationState::Open
            || receipt.manifest.is_some_and(|m| {
                index >= m.chunks
                    || receipt.captured_records + chunk.records.len() as u32 > m.records
            })
        {
            return Err(SyncError::Conflict);
        }
        sqlx::query("INSERT INTO shipping_sync_chunks(session_id,chunk_index,digest,records,payload) VALUES($1,$2,$3,$4,$5)").bind(receipt.id).bind(index as i32).bind(chunk.digest.as_slice()).bind(chunk.records.len() as i32).bind(payload).execute(&mut *self.0).await.map_err(failure)?;
        Ok(())
    }
    async fn chunks(&mut self, receipt: &Receipt) -> Result<Vec<Chunk>, SyncError> {
        let rows:Vec<(i32,Vec<u8>,Option<serde_json::Value>)>=sqlx::query_as("SELECT chunk_index,digest,payload FROM shipping_sync_chunks WHERE session_id=$1 ORDER BY chunk_index").bind(receipt.id).fetch_all(&mut *self.0).await.map_err(failure)?;
        let mut chunks = Vec::new();
        for (i, (index, digest, payload)) in rows.into_iter().enumerate() {
            if index as usize != i {
                return Err(SyncError::Incomplete);
            }
            let chunk = codec::decode_chunk::<StoredReferences>(
                payload.ok_or(SyncError::Conflict)?,
                receipt.source,
            )
            .map_err(encoding)?;
            if chunk.digest != bytes(digest)? {
                return Err(corrupt());
            }
            chunks.push(chunk);
        }
        Ok(chunks)
    }
    async fn seal(
        &mut self,
        receipt: &Receipt,
        manifest: Manifest,
        complete: bool,
    ) -> Result<(), SyncError> {
        if receipt.manifest.is_some_and(|m| m != manifest) {
            return Err(SyncError::Conflict);
        }
        sqlx::query("UPDATE shipping_sync_sessions SET manifest_chunks=$2,manifest_records=$3,manifest_digest=$4,raw_complete=raw_complete OR $5,state=CASE WHEN $5 AND state='OPEN' THEN 'READY' ELSE state END WHERE session_id=$1").bind(receipt.id).bind(manifest.chunks as i32).bind(manifest.records as i32).bind(manifest.digest.as_slice()).bind(complete).execute(&mut *self.0).await.map_err(failure)?;
        Ok(())
    }
    async fn state(
        &mut self,
        receipt: &Receipt,
        state: ApplicationState,
        error: Option<&str>,
    ) -> Result<(), SyncError> {
        sqlx::query("UPDATE shipping_sync_sessions SET state=$2,failure=$3 WHERE session_id=$1 AND state<>'APPLIED'").bind(receipt.id).bind(state.as_str()).bind(error).execute(&mut *self.0).await.map_err(failure)?;
        if state == ApplicationState::ReconcileRequired {
            sqlx::query("UPDATE shipping_sync_scopes SET dirty=true WHERE epoch=$1 AND scope=$2")
                .bind(receipt.begin.epoch)
                .bind(receipt.begin.scope.as_str())
                .execute(&mut *self.0)
                .await
                .map_err(failure)?;
        }
        Ok(())
    }
    async fn activate(
        &mut self,
        receipt: &Receipt,
        configuration_changed: bool,
    ) -> Result<(), SyncError> {
        let expected_sequence =
            receipt.begin.expected_sequence + i64::from(receipt.begin.mode == Mode::Delta);
        let counters:Option<(i64,i64)>=sqlx::query_as("UPDATE shipping_sync_epochs SET live_sequence=live_sequence+$3,configuration_sequence=configuration_sequence+$3 WHERE epoch=$1 AND live_sequence=$2 RETURNING live_sequence,configuration_sequence").bind(receipt.begin.epoch).bind(expected_sequence).bind(i64::from(configuration_changed)).fetch_optional(&mut *self.0).await.map_err(failure)?;
        let (sequence, configuration) = counters.ok_or(SyncError::Dirty)?;
        // A configuration-only snapshot cannot certify existing item dependencies.
        // Scopes without live membership can advance their own configuration fence.
        let generation: Option<i64> = sqlx::query_scalar(
            r#"
            UPDATE shipping_sync_scopes s
            SET generation = generation + 1,
                live_sequence = $4,
                configuration_sequence = CASE
                    WHEN $5 AND ($7 OR NOT EXISTS (
                        SELECT 1 FROM shipping_sync_claims c
                        WHERE c.epoch = s.epoch AND c.scope = s.scope
                            AND c.object_kind = 'ITEM' AND NOT c.tombstoned
                    )) THEN $6 ELSE configuration_sequence END,
                dirty = CASE
                    WHEN $5 AND ($7 OR NOT EXISTS (
                        SELECT 1 FROM shipping_sync_claims c
                        WHERE c.epoch = s.epoch AND c.scope = s.scope
                            AND c.object_kind = 'ITEM' AND NOT c.tombstoned
                    )) THEN false ELSE dirty END
            WHERE epoch = $1 AND scope = $2 AND generation = $3
            RETURNING generation
            "#,
        )
        .bind(receipt.begin.epoch)
        .bind(receipt.begin.scope.as_str())
        .bind(receipt.begin.expected_generation)
        .bind(sequence)
        .bind(receipt.begin.mode == Mode::Snapshot)
        .bind(configuration)
        .bind(receipt.begin.components[2])
        .fetch_optional(&mut *self.0)
        .await
        .map_err(failure)?;
        let generation = generation.ok_or(SyncError::Dirty)?;
        sqlx::query("UPDATE shipping_sync_sessions SET state='APPLIED',generation=$2,failure=NULL WHERE session_id=$1").bind(receipt.id).bind(generation).execute(&mut *self.0).await.map_err(failure)?;
        sqlx::query("INSERT INTO shipping_sync_changes(session_id,listing_source_id,epoch,generation) VALUES($1,$2,$3,$4)").bind(receipt.id).bind(receipt.source.into_uuid()).bind(receipt.begin.epoch).bind(generation).execute(&mut *self.0).await.map_err(failure)?;
        Ok(())
    }
    async fn claims(
        &mut self,
        binding: &Binding,
        scope: &ExternalKey,
    ) -> Result<Vec<Claim>, SyncError> {
        let rows=sqlx::query_as::<_,ClaimRow>("SELECT * FROM shipping_sync_claims WHERE epoch=$1 AND scope=$2 ORDER BY object_kind,external_key LIMIT 257 FOR UPDATE").bind(binding.epoch).bind(scope.as_str()).fetch_all(&mut *self.0).await.map_err(failure)?;
        if rows.len() > MAX_RECORDS as usize {
            return Err(SyncError::Limit);
        }
        rows.into_iter().map(|r| r.claim(binding)).collect()
    }
    async fn claim(
        &mut self,
        binding: &Binding,
        kind: ObjectKind,
        key: &ExternalKey,
    ) -> Result<Option<Claim>, SyncError> {
        sqlx::query_as::<_,ClaimRow>("SELECT * FROM shipping_sync_claims WHERE epoch=$1 AND object_kind=$2 AND external_key=$3 FOR UPDATE").bind(binding.epoch).bind(kind.as_str()).bind(key.as_str()).fetch_optional(&mut *self.0).await.map_err(failure)?.map(|r|r.claim(binding)).transpose()
    }
    async fn save_claim(&mut self, binding: &Binding, claim: &Claim) -> Result<(), SyncError> {
        let count: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM shipping_sync_claims WHERE epoch=$1 AND scope=$2",
        )
        .bind(binding.epoch)
        .bind(claim.scope.as_str())
        .fetch_one(&mut *self.0)
        .await
        .map_err(failure)?;
        if count >= i64::from(MAX_RECORDS)
            && self.claim(binding, claim.kind, &claim.key).await?.is_none()
        {
            return Err(SyncError::Limit);
        }
        sqlx::query("INSERT INTO shipping_sync_claims(epoch,object_kind,external_key,scope,observed_at,fresh_until,expires_at,applied,tombstoned) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9) ON CONFLICT(epoch,object_kind,external_key) DO UPDATE SET observed_at=$5,fresh_until=$6,expires_at=$7,applied=$8,tombstoned=$9 WHERE shipping_sync_claims.scope=$4").bind(binding.epoch).bind(claim.kind.as_str()).bind(claim.key.as_str()).bind(claim.scope.as_str()).bind(claim.window.observed_at).bind(claim.window.fresh_until).bind(claim.window.expires_at).bind(codec::encode_applied::<StoredReferences>(&claim.value).map_err(encoding)?).bind(claim.tombstoned).execute(&mut *self.0).await.map_err(failure)?;
        Ok(())
    }
    async fn mark_profile(
        &mut self,
        binding: &Binding,
        id: ShippingProfileId,
        window: ObservationWindow,
        changed: bool,
    ) -> Result<(), SyncError> {
        let (method, domain) = match &binding.integration {
            DeliveryIntegration::Source => (None, None),
            DeliveryIntegration::Provider { method, domain } => {
                (Some(method.as_str()), Some(domain.as_str()))
            }
        };
        sqlx::query("UPDATE shipping_profiles SET sync_epoch=$2,integration_method=$3,integration_domain=$4,verified_at=CASE WHEN $5 THEN $6 ELSE verified_at END,fresh_until=CASE WHEN $5 THEN $7 ELSE fresh_until END,expires_at=CASE WHEN $5 THEN $8 ELSE expires_at END WHERE shipping_profile_id=$1 AND listing_source_id=$9").bind(id.into_uuid()).bind(binding.epoch).bind(method).bind(domain).bind(changed).bind(window.observed_at).bind(window.fresh_until).bind(window.expires_at).bind(binding.source.into_uuid()).execute(&mut *self.0).await.map_err(failure)?;
        Ok(())
    }
    async fn mark_item(
        &mut self,
        binding: &Binding,
        _delivery: &ListingDelivery,
        key: &SourceListingId,
        owned: [bool; 4],
        window: ObservationWindow,
        changed: [bool; 4],
    ) -> Result<(), SyncError> {
        // Column identifiers are fixed adapter constants, never supplied by a caller.
        for (i, component) in ["selection", "inputs", "shipping", "collection"]
            .iter()
            .enumerate()
        {
            if !owned[i] {
                continue;
            }
            let sql = format!(
                "UPDATE product_listings SET delivery_{component}_epoch=$3,delivery_{component}_observed_at=CASE WHEN $4 THEN $5 ELSE delivery_{component}_observed_at END,delivery_{component}_fresh_until=CASE WHEN $4 THEN $6 ELSE delivery_{component}_fresh_until END,delivery_{component}_expires_at=CASE WHEN $4 THEN $7 ELSE delivery_{component}_expires_at END WHERE listing_source_id=$1 AND source_listing_id=$2"
            );
            sqlx::query(sqlx::AssertSqlSafe(sql))
                .bind(binding.source.into_uuid())
                .bind(key.as_ref())
                .bind(binding.epoch)
                .bind(changed[i])
                .bind(window.observed_at)
                .bind(window.fresh_until)
                .bind(window.expires_at)
                .execute(&mut *self.0)
                .await
                .map_err(failure)?;
        }
        if changed.iter().any(|c| *c) {
            // A conservative whole-configuration age, independent component expiry gates
            // below preserve explicit manager assertions when an acquired component expires.
            sqlx::query("UPDATE product_listings SET delivery_verified_at=LEAST(delivery_selection_observed_at,delivery_inputs_observed_at,delivery_shipping_observed_at,delivery_collection_observed_at,$3),delivery_fresh_until=LEAST(delivery_selection_fresh_until,delivery_inputs_fresh_until,delivery_shipping_fresh_until,delivery_collection_fresh_until),delivery_expires_at=NULL WHERE listing_source_id=$1 AND source_listing_id=$2 AND delivery_configuration IS NOT NULL").bind(binding.source.into_uuid()).bind(key.as_ref()).bind(window.observed_at).execute(&mut *self.0).await.map_err(failure)?;
        }
        Ok(())
    }
    async fn item_ownership(
        &mut self,
        binding: &Binding,
        key: &SourceListingId,
    ) -> Result<ItemOwnership, SyncError> {
        let row:Option<(Option<Uuid>,Option<Uuid>,Option<Uuid>,Option<Uuid>)>=sqlx::query_as("SELECT delivery_selection_epoch,delivery_inputs_epoch,delivery_shipping_epoch,delivery_collection_epoch FROM product_listings WHERE listing_source_id=$1 AND source_listing_id=$2").bind(binding.source.into_uuid()).bind(key.as_ref()).fetch_optional(&mut *self.0).await.map_err(failure)?;
        let Some((a, b, c, d)) = row else {
            return Ok(ItemOwnership {
                current: [false; 4],
                reclaimable: [false; 4],
            });
        };
        let retired:Vec<Uuid>=sqlx::query_scalar("SELECT epoch FROM shipping_sync_epochs WHERE epoch=ANY($1) AND NOT active AND listing_source_id=$2").bind([a,b,c,d].into_iter().flatten().collect::<Vec<_>>()).bind(binding.source.into_uuid()).fetch_all(&mut *self.0).await.map_err(failure)?;
        Ok(ItemOwnership {
            current: [a, b, c, d].map(|e| e == Some(binding.epoch)),
            reclaimable: [a, b, c, d].map(|e| e.is_some_and(|e| retired.contains(&e))),
        })
    }
    async fn default_owner(&mut self, binding: &Binding) -> Result<Option<Uuid>, SyncError> {
        sqlx::query_scalar::<_,Option<Uuid>>("SELECT CASE WHEN d.sync_epoch=$2 OR NOT e.active THEN d.sync_epoch END FROM shipping_source_defaults d LEFT JOIN shipping_sync_epochs e ON e.epoch=d.sync_epoch WHERE d.listing_source_id=$1").bind(binding.source.into_uuid()).bind(binding.epoch).fetch_optional(&mut *self.0).await.map(|r|r.flatten()).map_err(failure)
    }
    async fn mark_default(
        &mut self,
        binding: &Binding,
        window: ObservationWindow,
        changed: bool,
    ) -> Result<(), SyncError> {
        sqlx::query("UPDATE shipping_source_defaults SET sync_epoch=$2,sync_observed_at=CASE WHEN $3 THEN $4 ELSE sync_observed_at END,sync_fresh_until=CASE WHEN $3 THEN $5 ELSE sync_fresh_until END,sync_expires_at=CASE WHEN $3 THEN $6 ELSE sync_expires_at END WHERE listing_source_id=$1").bind(binding.source.into_uuid()).bind(binding.epoch).bind(changed).bind(window.observed_at).bind(window.fresh_until).bind(window.expires_at).execute(&mut *self.0).await.map_err(failure)?;
        Ok(())
    }
    async fn pending(
        &mut self,
        binding: &Binding,
        after: Option<Uuid>,
        limit: u32,
    ) -> Result<Vec<Uuid>, SyncError> {
        sqlx::query_scalar("SELECT r.session_id FROM shipping_sync_sessions r JOIN shipping_sync_scopes s ON s.epoch=r.epoch AND s.scope=r.scope JOIN shipping_sync_epochs e ON e.epoch=s.epoch WHERE r.epoch=$1 AND ((r.state IN ('READY','WAITING_DEPENDENCY','RECONCILE_REQUIRED') AND r.expires_at>clock_timestamp()) OR (r.state='APPLIED' AND r.generation=s.generation AND (s.dirty OR s.configuration_sequence<>e.configuration_sequence))) AND ($2::uuid IS NULL OR r.session_id>$2) ORDER BY r.session_id LIMIT $3").bind(binding.epoch).bind(after).bind(i64::from(limit)).fetch_all(&mut *self.0).await.map_err(failure)
    }
}
