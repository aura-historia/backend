mod codec;
use application::error::box_error;
use codec::*;
use geo_service::{geocoding::GeocodingFailure, resolution::*};
use platform_postgres::SqlxTransaction;
use sqlx::{FromRow, PgConnection};
use time::{Duration, OffsetDateTime};
use uuid::Uuid;

#[derive(Debug, Clone, Copy, Default)]
pub struct SqlxResolutionRepositoryFactory;
impl ResolutionRepositoryFactory<SqlxTransaction> for SqlxResolutionRepositoryFactory {
    fn resolutions<'tx>(
        &'tx self,
        tx: &'tx mut SqlxTransaction,
    ) -> impl ResolutionRepository + 'tx {
        Self::on_connection(tx.connection())
    }
    fn resolution_reader<'tx>(
        &'tx self,
        tx: &'tx mut SqlxTransaction,
    ) -> impl ResolutionReader + 'tx {
        Self::on_connection(tx.connection())
    }
}
impl SqlxResolutionRepositoryFactory {
    /// Compose multiple service-owned capabilities on the same authoritative transaction.
    pub fn on_connection(
        connection: &mut PgConnection,
    ) -> impl ResolutionRepository + ResolutionReader + '_ {
        Repository { connection }
    }
}
struct Repository<'a> {
    connection: &'a mut PgConnection,
}
fn sql_error(e: sqlx::Error) -> ResolutionError {
    ResolutionError::Storage(box_error(e))
}
fn revision(v: u64) -> Result<i64, ResolutionError> {
    if v == 0 {
        return Err(ResolutionError::InvalidInput);
    }
    i64::try_from(v).map_err(invalid)
}
fn work(id: Uuid, generation: i64) -> Result<ResolutionWork, ResolutionError> {
    Ok(ResolutionWork {
        id: ResolutionId::try_from(id).map_err(invalid)?,
        generation: u64::try_from(generation).map_err(invalid)?,
    })
}

#[derive(FromRow)]
struct ResolutionRow {
    resolution_id: Uuid,
    fingerprint: String,
    profile: String,
    policy_version: String,
    scope_key: String,
    input: serde_json::Value,
    generation: i64,
    status: String,
    attempts: i32,
    lease_token: Option<Uuid>,
    lease_until: Option<OffsetDateTime>,
    next_attempt_at: OffsetDateTime,
    evidence: Option<serde_json::Value>,
    valid_until: Option<OffsetDateTime>,
    last_failure: Option<String>,
}
impl ResolutionRow {
    fn input(&self) -> Result<ResolutionInput, ResolutionError> {
        let input: ResolutionInput = decode::<InputData>(self.input.clone())?.try_into()?;
        if self.policy_version != POLICY_VERSION
            || input.fingerprint(&self.profile) != self.fingerprint
            || input.budget_key() != self.scope_key
            || self.generation <= 0
            || !(0..=5).contains(&self.attempts)
            || ResolutionStatus::from_code(&self.status).is_none()
            || (matches!(self.status.as_str(), "ACCEPTED" | "AMBIGUOUS" | "NO_MATCH")
                != self.evidence.is_some())
            || self.evidence.is_some() != self.valid_until.is_some()
            || self.last_failure.as_deref().is_some_and(|code| {
                GeocodingFailure::from_code(code).is_none() && code != "LEASE_ATTEMPTS_EXHAUSTED"
            })
        {
            return Err(corrupt());
        }
        Ok(input)
    }
    fn evidence(
        &self,
        input: &ResolutionInput,
    ) -> Result<Option<ResolutionEvidence>, ResolutionError> {
        self.evidence
            .clone()
            .map(|v| {
                let e: ResolutionEvidence = decode::<EvidenceData>(v)?.try_into()?;
                if e.status.as_str() != self.status
                    || e.candidates.iter().any(|c| {
                        c.profile != self.profile
                            || (c.acceptable && !candidate_compatible(input, &c.geography))
                    })
                {
                    return Err(corrupt());
                }
                Ok(e)
            })
            .transpose()
    }
}
#[derive(FromRow)]
struct Controls {
    profile: String,
    enabled: bool,
    storage_permitted: bool,
    daily_limit: i32,
    scope_daily_limit: i32,
    concurrency_limit: i32,
    scope_concurrency_limit: i32,
    calls: i32,
    consecutive_failures: i32,
    open_until: Option<OffsetDateTime>,
}
impl Repository<'_> {
    async fn clock(&mut self) -> Result<OffsetDateTime, ResolutionError> {
        sqlx::query_scalar("SELECT clock_timestamp()")
            .fetch_one(&mut *self.connection)
            .await
            .map_err(sql_error)
    }
    async fn row(&mut self, id: ResolutionId) -> Result<Option<ResolutionRow>, ResolutionError> {
        // Resolution keys are immutable. Keep foreign-key journal inserts compatible
        // with coordination so a concurrent source detach cannot form a lock cycle.
        sqlx::query_as(
            r#"
            SELECT
            resolution_id,fingerprint,profile,policy_version,scope_key,input,generation,status,attempts,lease_token,lease_until,next_attempt_at,evidence,valid_until,last_failure
            FROM geo_resolutions WHERE resolution_id=$1 FOR NO KEY UPDATE
            "#,
        ).bind(id.into_uuid()).fetch_optional(&mut *self.connection).await.map_err(sql_error)
    }
    async fn signal(
        &mut self,
        w: ResolutionWork,
        kind: &str,
        subject: Option<ResolutionSubject>,
    ) -> Result<(), ResolutionError> {
        sqlx::query(
            r#"
            INSERT INTO
            geo_resolution_changes(change_id,resolution_id,generation,change_kind,subject_kind,subject_id)
            VALUES ($1,$2,$3,$4,$5,$6)
            "#,
        ).bind(Uuid::now_v7()).bind(w.id.into_uuid()).bind(revision(w.generation)?).bind(kind).bind(subject.map(|s|s.kind().as_str())).bind(subject.map(|s|s.id())).execute(&mut *self.connection).await.map_err(sql_error)?;
        Ok(())
    }
    async fn wake(&mut self, w: ResolutionWork) -> Result<(), ResolutionError> {
        sqlx::query("INSERT INTO geo_resolution_wakeups(resolution_id,generation) VALUES ($1,$2) ON CONFLICT DO NOTHING").bind(w.id.into_uuid()).bind(revision(w.generation)?).execute(&mut *self.connection).await.map_err(sql_error)?;
        Ok(())
    }
    async fn controls(&mut self, profile: &str) -> Result<Controls, ResolutionError> {
        // Serialize reservations/completions without blocking foreign-key checks
        // when another transaction binds input to this unchanged provider identity.
        sqlx::query_as(
            r#"
            SELECT
            profile,enabled,storage_permitted,daily_limit,scope_daily_limit,concurrency_limit,scope_concurrency_limit,calls,consecutive_failures,open_until
            FROM geo_resolution_provider_controls WHERE profile=$1 FOR NO KEY UPDATE
            "#,
        ).bind(profile).fetch_optional(&mut *self.connection).await.map_err(sql_error)?.ok_or_else(corrupt)
    }
}
#[async_trait::async_trait]
impl ResolutionRepository for Repository<'_> {
    async fn bind(
        &mut self,
        input: &ResolutionInput,
        b: ResolutionBinding,
    ) -> Result<ResolutionId, ResolutionError> {
        if b.subject.kind() != input.purpose() {
            return Err(ResolutionError::InvalidInput);
        }
        let profile: String = sqlx::query_scalar(
            "SELECT profile FROM geo_resolution_provider_controls WHERE is_default",
        )
        .fetch_optional(&mut *self.connection)
        .await
        .map_err(sql_error)?
        .ok_or_else(corrupt)?;
        let fingerprint = input.fingerprint(&profile);
        let id = ResolutionId::new();
        let (stored_id,generation):(Uuid,i64)=sqlx::query_as(
            r#"
            INSERT INTO geo_resolutions(resolution_id,fingerprint,profile,policy_version,scope_key,input) VALUES
            ($1,$2,$3,$4,$5,$6) ON CONFLICT(fingerprint) DO UPDATE SET generation=geo_resolutions.generation
            RETURNING resolution_id,generation
            "#,
        )
            .bind(id.into_uuid()).bind(fingerprint).bind(profile).bind(POLICY_VERSION).bind(input.budget_key()).bind(encode(&InputData::from(input))?).fetch_one(&mut *self.connection).await.map_err(sql_error)?;
        let w = work(stored_id, generation)?;
        let row = self.row(w.id).await?.ok_or_else(corrupt)?;
        let stored_input = row.input()?;
        let previous: Option<(Uuid, i64)> = sqlx::query_as(
            "SELECT resolution_id,input_revision FROM geo_resolution_bindings WHERE subject_kind=$1 AND subject_id=$2",
        )
        .bind(b.subject.kind().as_str())
        .bind(b.subject.id())
        .fetch_optional(&mut *self.connection)
        .await
        .map_err(sql_error)?;
        let source_changed = previous != Some((stored_id, revision(b.input_revision)?));
        let count = sqlx::query(
            r#"
            INSERT INTO geo_resolution_bindings(subject_kind,subject_id,resolution_id,input_revision,guard)
            VALUES ($1,$2,$3,$4,$5) ON CONFLICT(subject_kind,subject_id) DO UPDATE SET
            resolution_id=EXCLUDED.resolution_id,input_revision=EXCLUDED.input_revision,guard=EXCLUDED.guard,selected_candidate=CASE
            WHEN geo_resolution_bindings.resolution_id=EXCLUDED.resolution_id AND
            geo_resolution_bindings.input_revision=EXCLUDED.input_revision THEN
            geo_resolution_bindings.selected_candidate END,selected_generation=CASE WHEN
            geo_resolution_bindings.resolution_id=EXCLUDED.resolution_id AND
            geo_resolution_bindings.input_revision=EXCLUDED.input_revision THEN
            geo_resolution_bindings.selected_generation END,reviewed_actor=CASE WHEN
            geo_resolution_bindings.resolution_id=EXCLUDED.resolution_id AND
            geo_resolution_bindings.input_revision=EXCLUDED.input_revision THEN
            geo_resolution_bindings.reviewed_actor END WHERE geo_resolution_bindings.guard<=EXCLUDED.guard AND
            geo_resolution_bindings.input_revision<=EXCLUDED.input_revision
            "#,
        )
            .bind(b.subject.kind().as_str()).bind(b.subject.id()).bind(stored_id).bind(revision(b.input_revision)?).bind(revision(b.guard)?).execute(&mut *self.connection).await.map_err(sql_error)?.rows_affected();
        if count != 1 {
            return Err(ResolutionError::Conflict);
        }
        if let Some((old, _)) = previous
            && source_changed
        {
            let g: i64 =
                sqlx::query_scalar("SELECT generation FROM geo_resolutions WHERE resolution_id=$1")
                    .bind(old)
                    .fetch_one(&mut *self.connection)
                    .await
                    .map_err(sql_error)?;
            self.signal(work(old, g)?, "INVALIDATED", Some(b.subject))
                .await?;
        }
        let now = self.clock().await?;
        if source_changed && row.valid_until.is_some_and(|until| until > now) {
            let permitted: bool = sqlx::query_scalar(
                "SELECT storage_permitted FROM geo_resolution_provider_controls WHERE profile=$1",
            )
            .bind(&row.profile)
            .fetch_one(&mut *self.connection)
            .await
            .map_err(sql_error)?;
            if permitted {
                row.evidence(&stored_input)?;
                // A cached result has no new completion or CDC wake-up. Notify
                // dependents when it becomes current for a new source binding.
                self.signal(w, "RESOLVED", Some(b.subject)).await?;
            }
        }
        self.wake(w).await?;
        tracing::info!(
            metric = "geo_resolution_input",
            reused = stored_id != id.into_uuid(),
            method = "GEOCODER",
            policy = POLICY_VERSION
        );
        Ok(w.id)
    }
    async fn detach(&mut self, subject: ResolutionSubject) -> Result<(), ResolutionError> {
        let old: Option<(Uuid, i64)> = sqlx::query_as(
            r#"
            DELETE FROM geo_resolution_bindings WHERE subject_kind=$1 AND subject_id=$2 RETURNING
            resolution_id,(SELECT generation FROM geo_resolutions WHERE
            resolution_id=geo_resolution_bindings.resolution_id)
            "#,
        )
        .bind(subject.kind().as_str())
        .bind(subject.id())
        .fetch_optional(&mut *self.connection)
        .await
        .map_err(sql_error)?;
        if let Some((id, g)) = old {
            self.signal(work(id, g)?, "INVALIDATED", Some(subject))
                .await?;
        }
        Ok(())
    }
    async fn claim(
        &mut self,
        w: ResolutionWork,
        profile: &str,
        allow_provider: bool,
    ) -> Result<ClaimOutcome, ResolutionError> {
        let mut control = self.controls(profile).await?;
        let Some(r) = self.row(w.id).await? else {
            return Err(ResolutionError::Missing);
        };
        let input = r.input()?;
        if w.generation != r.generation as u64 {
            return Ok(ClaimOutcome::Complete);
        }
        if r.profile != control.profile {
            return Err(ResolutionError::Conflict);
        }
        let now = self.clock().await?;
        let bound: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM geo_resolution_bindings WHERE resolution_id=$1)",
        )
        .bind(w.id.into_uuid())
        .fetch_one(&mut *self.connection)
        .await
        .map_err(sql_error)?;
        if !bound {
            return Ok(ClaimOutcome::Complete);
        }
        if r.valid_until.is_some_and(|until| until > now) && control.storage_permitted {
            r.evidence(&input)?;
            return Ok(ClaimOutcome::Complete);
        }
        if r.status != "PENDING"
            || !allow_provider
            || !control.enabled
            || !control.storage_permitted
            || control.open_until.is_some_and(|until| until > now)
            || r.attempts >= 5
            || r.next_attempt_at > now
            || r.lease_until.is_some_and(|until| until > now)
        {
            return Ok(ClaimOutcome::Deferred);
        }
        sqlx::query(
            r#"
            UPDATE geo_resolution_provider_controls SET calls=CASE WHEN budget_day=$2 THEN calls ELSE 0
            END,budget_day=$2 WHERE profile=$1
            "#,
        ).bind(profile).bind(now.date()).execute(&mut *self.connection).await.map_err(sql_error)?;
        control.calls = sqlx::query_scalar(
            "SELECT calls FROM geo_resolution_provider_controls WHERE profile=$1",
        )
        .bind(profile)
        .fetch_one(&mut *self.connection)
        .await
        .map_err(sql_error)?;
        sqlx::query(
            r#"
            INSERT INTO geo_resolution_scope_budgets(profile,scope_key,budget_day,calls) VALUES($1,$2,$3,0) ON
            CONFLICT(profile,scope_key) DO UPDATE SET calls=CASE WHEN
            geo_resolution_scope_budgets.budget_day=EXCLUDED.budget_day THEN geo_resolution_scope_budgets.calls
            ELSE 0 END,budget_day=EXCLUDED.budget_day
            "#,
        ).bind(profile).bind(&r.scope_key).bind(now.date()).execute(&mut *self.connection).await.map_err(sql_error)?;
        let scope_calls: i32 = sqlx::query_scalar(
            "SELECT calls FROM geo_resolution_scope_budgets WHERE profile=$1 AND scope_key=$2",
        )
        .bind(profile)
        .bind(&r.scope_key)
        .fetch_one(&mut *self.connection)
        .await
        .map_err(sql_error)?;
        let (active,scope_active):(i64,i64)=sqlx::query_as("SELECT count(*),count(*) FILTER(WHERE scope_key=$2) FROM geo_resolutions WHERE profile=$1 AND lease_until>$3").bind(profile).bind(&r.scope_key).bind(now).fetch_one(&mut *self.connection).await.map_err(sql_error)?;
        if control.calls >= control.daily_limit
            || scope_calls >= control.scope_daily_limit
            || active >= i64::from(control.concurrency_limit)
            || scope_active >= i64::from(control.scope_concurrency_limit)
        {
            return Ok(ClaimOutcome::Deferred);
        }
        // A crashed fifth reservation is exhausted once its lease expires.
        let token = Uuid::now_v7();
        sqlx::query("UPDATE geo_resolutions SET lease_token=$2,lease_until=$3,attempts=attempts+1 WHERE resolution_id=$1").bind(w.id.into_uuid()).bind(token).bind(now+Duration::seconds(60)).execute(&mut *self.connection).await.map_err(sql_error)?;
        sqlx::query("UPDATE geo_resolution_provider_controls SET calls=calls+1 WHERE profile=$1")
            .bind(profile)
            .execute(&mut *self.connection)
            .await
            .map_err(sql_error)?;
        sqlx::query("UPDATE geo_resolution_scope_budgets SET calls=calls+1 WHERE profile=$1 AND scope_key=$2").bind(profile).bind(r.scope_key).execute(&mut *self.connection).await.map_err(sql_error)?;
        Ok(ClaimOutcome::Claimed(Box::new(ResolutionClaim {
            work: w,
            token,
            input,
            profile: r.profile,
            attempt: (r.attempts + 1) as u32,
        })))
    }
    async fn complete(
        &mut self,
        c: &ResolutionClaim,
        completion: ResolutionCompletion,
    ) -> Result<bool, ResolutionError> {
        let control = self.controls(&c.profile).await?;
        let Some(r) = self.row(c.work.id).await? else {
            return Err(ResolutionError::Missing);
        };
        r.input()?;
        let now = self.clock().await?;
        if r.generation as u64 != c.work.generation
            || r.lease_token != Some(c.token)
            || r.lease_until.is_none_or(|until| until <= now)
        {
            // A forced generation change keeps the old reservation until its call
            // finishes or the lease expires. Only that exact call may release it;
            // it cannot apply evidence or release a newer claim's reservation.
            if r.generation as u64 != c.work.generation && r.lease_token == Some(c.token) {
                sqlx::query("UPDATE geo_resolutions SET lease_token=NULL,lease_until=NULL WHERE resolution_id=$1")
                    .bind(c.work.id.into_uuid()).execute(&mut *self.connection).await.map_err(sql_error)?;
            }
            return Ok(false);
        }
        let bound: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM geo_resolution_bindings WHERE resolution_id=$1)",
        )
        .bind(c.work.id.into_uuid())
        .fetch_one(&mut *self.connection)
        .await
        .map_err(sql_error)?;
        if !bound {
            sqlx::query("UPDATE geo_resolutions SET lease_token=NULL,lease_until=NULL WHERE resolution_id=$1").bind(c.work.id.into_uuid()).execute(&mut *self.connection).await.map_err(sql_error)?;
            return Ok(true);
        }
        let completion = if !control.storage_permitted {
            ResolutionCompletion::Failure(GeocodingFailure::StorageNotPermitted)
        } else if !control.enabled {
            ResolutionCompletion::Failure(GeocodingFailure::Configuration)
        } else {
            completion
        };
        match completion {
            ResolutionCompletion::Evidence(e) => {
                let retention = e
                    .status
                    .retention_period()
                    .ok_or(ResolutionError::InvalidInput)?;
                let data = encode(&EvidenceData::from(&e))?;
                let _: ResolutionEvidence = decode::<EvidenceData>(data.clone())?.try_into()?;
                if e.candidates.iter().any(|v| v.profile != c.profile) {
                    return Err(ResolutionError::InvalidInput);
                }
                sqlx::query(
                    r#"
                    UPDATE geo_resolutions SET
                    status=$2,evidence=$3,valid_until=$4,last_failure=NULL,lease_token=NULL,lease_until=NULL WHERE
                    resolution_id=$1
                    "#,
                ).bind(c.work.id.into_uuid()).bind(e.status.as_str()).bind(data).bind(now+retention).execute(&mut *self.connection).await.map_err(sql_error)?;
                sqlx::query("UPDATE geo_resolution_provider_controls SET consecutive_failures=0,open_until=NULL WHERE profile=$1").bind(&c.profile).execute(&mut *self.connection).await.map_err(sql_error)?;
                self.signal(c.work, "RESOLVED", None).await?;
                Ok(true)
            }
            ResolutionCompletion::Failure(kind) => {
                let status = ResolutionStatus::after_failure(kind, r.attempts as u32).as_str();
                let backoff = retry_delay(r.attempts as u32);
                sqlx::query(
                    r#"
                    UPDATE geo_resolutions SET
                    status=$2,last_failure=$3,next_attempt_at=$4,lease_token=NULL,lease_until=NULL,evidence=NULL,valid_until=NULL
                    WHERE resolution_id=$1
                    "#,
                ).bind(c.work.id.into_uuid()).bind(status).bind(kind.as_str()).bind(now+backoff).execute(&mut *self.connection).await.map_err(sql_error)?;
                let failures = control.consecutive_failures.saturating_add(1).min(100000);
                sqlx::query("UPDATE geo_resolution_provider_controls SET consecutive_failures=$2,open_until=$3 WHERE profile=$1").bind(&c.profile).bind(failures).bind(if failures>=3 {Some(now+Duration::minutes(5))} else {control.open_until}).execute(&mut *self.connection).await.map_err(sql_error)?;
                // Operational/configuration failures keep transport failure custody.
                Ok(false)
            }
        }
    }
    async fn recover_pending(&mut self, limit: u32) -> Result<u32, ResolutionError> {
        if !(1..=100).contains(&limit) {
            return Err(ResolutionError::InvalidInput);
        }
        let now = self.clock().await?;
        sqlx::query(
            r#"
            WITH exhausted AS (
                SELECT resolution_id FROM geo_resolutions
                WHERE status='PENDING' AND attempts=5 AND lease_until<=$1
                ORDER BY lease_until LIMIT $2 FOR NO KEY UPDATE SKIP LOCKED
            )
            UPDATE geo_resolutions r SET
            status='FAILED',last_failure='LEASE_ATTEMPTS_EXHAUSTED',lease_token=NULL,lease_until=NULL
            FROM exhausted e WHERE r.resolution_id=e.resolution_id
            "#,
        ).bind(now).bind(i64::from(limit)).execute(&mut *self.connection).await.map_err(sql_error)?;
        let oldest:Option<OffsetDateTime>=sqlx::query_scalar(
            r#"
            SELECT min(pending_since) FROM geo_resolutions WHERE status='PENDING' AND EXISTS(SELECT 1 FROM
            geo_resolution_bindings b WHERE b.resolution_id=geo_resolutions.resolution_id)
            "#,
        ).fetch_one(&mut *self.connection).await.map_err(sql_error)?;
        tracing::info!(
            metric = "geo_resolution_oldest_pending",
            age_seconds = oldest
                .map(|v| (now - v).whole_seconds().max(0))
                .unwrap_or(0)
        );
        let rows: Vec<ResolutionRow> = sqlx::query_as(r#"
            SELECT r.*
            FROM geo_resolutions r
            JOIN geo_resolution_provider_controls c ON c.profile=r.profile
            WHERE c.is_default AND r.status='PENDING' AND r.attempts<5
                AND r.next_attempt_at<=$1 AND (r.lease_until IS NULL OR r.lease_until<=$1)
                AND c.enabled AND c.storage_permitted
                AND (c.open_until IS NULL OR c.open_until<=$1)
                AND (c.budget_day<>($1 AT TIME ZONE 'UTC')::date OR c.calls<c.daily_limit)
                AND NOT EXISTS (
                    SELECT 1 FROM geo_resolution_scope_budgets s
                    WHERE s.profile=r.profile AND s.scope_key=r.scope_key
                        AND s.budget_day=($1 AT TIME ZONE 'UTC')::date
                        AND s.calls>=c.scope_daily_limit
                )
                AND EXISTS(SELECT 1 FROM geo_resolution_bindings b WHERE b.resolution_id=r.resolution_id)
            ORDER BY r.next_attempt_at,r.created LIMIT $2 FOR NO KEY UPDATE OF r SKIP LOCKED
        "#).bind(now).bind(i64::from(limit))
            .fetch_all(&mut *self.connection).await.map_err(sql_error)?;
        let count = rows.len() as u32;
        for row in rows {
            row.input()?;
            let id = ResolutionId::try_from(row.resolution_id).map_err(invalid)?;
            let next = row.generation.checked_add(1).ok_or_else(corrupt)?;
            let w = work(id.into_uuid(), next)?;
            // A new immutable journal row also repairs a previously missed CDC event.
            // Keep attempts, pending age and all budget reservations across recovery.
            sqlx::query(
                r#"
                UPDATE geo_resolutions SET generation=$2,lease_token=NULL,lease_until=NULL,
                next_attempt_at=$3 WHERE resolution_id=$1
                "#,
            )
            .bind(id.into_uuid())
            .bind(next)
            .bind(now)
            .execute(&mut *self.connection)
            .await
            .map_err(sql_error)?;
            self.signal(w, "REQUEUED", None).await?;
            self.wake(w).await?;
        }
        Ok(count)
    }
    async fn expire(&mut self, limit: u32) -> Result<u32, ResolutionError> {
        if !(1..=100).contains(&limit) {
            return Err(ResolutionError::InvalidInput);
        }
        let now = self.clock().await?;
        let rows:Vec<(Uuid,i64,bool)>=sqlx::query_as(
            r#"
            SELECT r.resolution_id,r.generation,c.storage_permitted FROM geo_resolutions r JOIN
            geo_resolution_provider_controls c ON c.profile=r.profile WHERE r.evidence IS NOT NULL AND
            (r.valid_until<=$1 OR NOT c.storage_permitted OR NOT EXISTS(SELECT 1 FROM geo_resolution_bindings b
            WHERE b.resolution_id=r.resolution_id)) ORDER BY r.valid_until LIMIT $2 FOR NO KEY UPDATE OF r SKIP LOCKED
            "#,
        ).bind(now).bind(i64::from(limit)).fetch_all(&mut *self.connection).await.map_err(sql_error)?;
        let count = rows.len() as u32;
        for (id, g, permitted) in rows {
            let next = g.checked_add(1).ok_or_else(corrupt)?;
            let w = work(id, next)?;
            sqlx::query(
                r#"
                UPDATE geo_resolutions SET
                evidence=NULL,valid_until=NULL,lease_token=NULL,lease_until=NULL,attempts=0,generation=$2,status=$3,next_attempt_at=$4,pending_since=$4,last_failure=$5
                WHERE resolution_id=$1
                "#,
            ).bind(id).bind(next).bind(if permitted {"PENDING"} else {"BLOCKED"}).bind(now).bind(if permitted {None} else {Some("STORAGE_NOT_PERMITTED")}).execute(&mut *self.connection).await.map_err(sql_error)?;
            sqlx::query(
                r#"
                UPDATE geo_resolution_bindings SET
                selected_candidate=NULL,selected_generation=NULL,reviewed_actor=NULL WHERE resolution_id=$1
                "#,
            ).bind(id).execute(&mut *self.connection).await.map_err(sql_error)?;
            self.signal(w, "INVALIDATED", None).await?;
            self.wake(w).await?;
        }
        Ok(count)
    }
    async fn select(
        &mut self,
        b: ResolutionBinding,
        w: ResolutionWork,
        index: usize,
        actor: &str,
    ) -> Result<(), ResolutionError> {
        let r = self.row(w.id).await?.ok_or(ResolutionError::Missing)?;
        let input = r.input()?;
        let now = self.clock().await?;
        let permitted: bool = sqlx::query_scalar(
            "SELECT storage_permitted FROM geo_resolution_provider_controls WHERE profile=$1",
        )
        .bind(&r.profile)
        .fetch_one(&mut *self.connection)
        .await
        .map_err(sql_error)?;
        if r.generation as u64 != w.generation
            || !permitted
            || r.valid_until.is_none_or(|until| until <= now)
        {
            return Err(ResolutionError::Conflict);
        }
        let e = r.evidence(&input)?.ok_or(ResolutionError::Conflict)?;
        let c = e
            .candidates
            .get(index)
            .ok_or(ResolutionError::InvalidInput)?;
        if !c.acceptable || !candidate_compatible(&input, &c.geography) {
            return Err(ResolutionError::InvalidInput);
        }
        let already:bool=sqlx::query_scalar(
            r#"
            SELECT EXISTS(SELECT 1 FROM geo_resolution_bindings WHERE subject_kind=$1 AND subject_id=$2 AND
            input_revision=$3 AND guard=$4 AND resolution_id=$5 AND selected_candidate=$6 AND
            selected_generation=$7)
            "#,
        )
            .bind(b.subject.kind().as_str()).bind(b.subject.id()).bind(revision(b.input_revision)?).bind(revision(b.guard)?).bind(w.id.into_uuid()).bind(index as i32).bind(revision(w.generation)?).fetch_one(&mut *self.connection).await.map_err(sql_error)?;
        if already {
            return Ok(());
        }
        let count=sqlx::query(
            r#"
            UPDATE geo_resolution_bindings SET selected_candidate=$6,selected_generation=$7,reviewed_actor=$8
            WHERE subject_kind=$1 AND subject_id=$2 AND input_revision=$3 AND guard=$4 AND resolution_id=$5
            "#,
        ).bind(b.subject.kind().as_str()).bind(b.subject.id()).bind(revision(b.input_revision)?).bind(revision(b.guard)?).bind(w.id.into_uuid()).bind(i32::try_from(index).map_err(invalid)?).bind(revision(w.generation)?).bind(actor).execute(&mut *self.connection).await.map_err(sql_error)?.rows_affected();
        if count != 1 {
            return Err(ResolutionError::Conflict);
        }
        self.signal(w, "REVIEWED", Some(b.subject)).await?;
        Ok(())
    }
    async fn requeue(
        &mut self,
        b: ResolutionBinding,
        expected: ResolutionWork,
        force: bool,
    ) -> Result<(), ResolutionError> {
        let Some(view) = self.inspect(b.subject).await? else {
            return Err(ResolutionError::Missing);
        };
        if view.input_revision != b.input_revision || view.guard != b.guard || view.work != expected
        {
            return Err(ResolutionError::Conflict);
        }
        let bound:Option<Uuid>=sqlx::query_scalar(
            r#"
            SELECT subject_id FROM geo_resolution_bindings WHERE subject_kind=$1 AND subject_id=$2 AND
            resolution_id=$3 AND input_revision=$4 AND guard=$5 FOR UPDATE
            "#,
        )
            .bind(b.subject.kind().as_str()).bind(b.subject.id()).bind(expected.id.into_uuid()).bind(revision(b.input_revision)?).bind(revision(b.guard)?).fetch_optional(&mut *self.connection).await.map_err(sql_error)?;
        if bound.is_none() {
            return Err(ResolutionError::Conflict);
        }
        if view.evidence.is_some() && !force {
            return Ok(());
        }
        let next = view.work.generation.checked_add(1).ok_or_else(corrupt)?;
        let w = ResolutionWork {
            id: view.work.id,
            generation: next,
        };
        // Explicit re-resolution invalidates prior derivatives and review selections.
        sqlx::query(
            r#"
            UPDATE geo_resolutions SET
            generation=$2,status='PENDING',pending_since=clock_timestamp(),attempts=0,last_failure=NULL,
            next_attempt_at=clock_timestamp(),evidence=NULL,valid_until=NULL
            WHERE resolution_id=$1
            "#,
        ).bind(w.id.into_uuid()).bind(revision(next)?).execute(&mut *self.connection).await.map_err(sql_error)?;
        sqlx::query(
            r#"
            UPDATE geo_resolution_bindings SET
            selected_candidate=NULL,selected_generation=NULL,reviewed_actor=NULL WHERE resolution_id=$1
            "#,
        ).bind(w.id.into_uuid()).execute(&mut *self.connection).await.map_err(sql_error)?;
        self.signal(w, "REQUEUED", None).await?;
        self.signal(w, "INVALIDATED", None).await?;
        self.wake(w).await?;
        Ok(())
    }
}

#[async_trait::async_trait]
impl ResolutionReader for Repository<'_> {
    async fn inspect_many(
        &mut self,
        subjects: &[ResolutionSubject],
    ) -> Result<std::collections::HashMap<Uuid, ResolutionView>, ResolutionError> {
        if subjects.len() > 100 {
            return Err(ResolutionError::InvalidInput);
        }
        let kinds = subjects
            .iter()
            .map(|s| s.kind().as_str())
            .collect::<Vec<_>>();
        let ids = subjects.iter().map(|s| s.id()).collect::<Vec<_>>();
        // Discover immutable resolution/profile identities without locks, then use
        // the provider -> resolution -> binding order shared by claim/completion.
        // Recheck authority after waits; multiple locations may share a resolution.
        let rows = sqlx::query_as::<_, ResolutionRow>(r#"
            SELECT DISTINCT r.* FROM geo_resolutions r JOIN geo_resolution_bindings b USING(resolution_id)
            JOIN UNNEST($1::text[],$2::uuid[]) requested(kind,id) ON b.subject_kind=requested.kind AND b.subject_id=requested.id
            ORDER BY r.resolution_id
        "#).bind(&kinds).bind(&ids).fetch_all(&mut *self.connection).await.map_err(sql_error)?;
        let profiles = rows.iter().map(|r| r.profile.clone()).collect::<Vec<_>>();
        let controls: Vec<(String,bool)> = sqlx::query_as("SELECT profile,storage_permitted FROM geo_resolution_provider_controls WHERE profile=ANY($1) ORDER BY profile FOR SHARE")
            .bind(profiles).fetch_all(&mut *self.connection).await.map_err(sql_error)?;
        let controls = controls
            .into_iter()
            .collect::<std::collections::HashMap<_, _>>();
        let resolution_ids = rows.iter().map(|r| r.resolution_id).collect::<Vec<_>>();
        // DISTINCT queries cannot use row locking; lock the referenced authority separately.
        sqlx::query("SELECT resolution_id FROM geo_resolutions WHERE resolution_id=ANY($1) ORDER BY resolution_id FOR NO KEY UPDATE")
            .bind(&resolution_ids).fetch_all(&mut *self.connection).await.map_err(sql_error)?;
        let rows = sqlx::query_as::<_, ResolutionRow>(
            "SELECT * FROM geo_resolutions WHERE resolution_id=ANY($1) ORDER BY resolution_id",
        )
        .bind(&resolution_ids)
        .fetch_all(&mut *self.connection)
        .await
        .map_err(sql_error)?;
        let bindings: Vec<(Uuid,Uuid,i64,i64,Option<i32>,Option<i64>)> = sqlx::query_as(r#"
            SELECT b.subject_id,b.resolution_id,b.input_revision,b.guard,b.selected_candidate,b.selected_generation
            FROM geo_resolution_bindings b JOIN UNNEST($1::text[],$2::uuid[]) requested(kind,id)
            ON b.subject_kind=requested.kind AND b.subject_id=requested.id
            WHERE b.resolution_id=ANY($3) ORDER BY b.subject_kind,b.subject_id FOR SHARE OF b
        "#).bind(kinds).bind(ids).bind(resolution_ids).fetch_all(&mut *self.connection).await.map_err(sql_error)?;
        let rows = rows
            .into_iter()
            .map(|r| (r.resolution_id, r))
            .collect::<std::collections::HashMap<_, _>>();
        let now = self.clock().await?;
        let mut result = std::collections::HashMap::new();
        for (subject, id, input_revision, guard, selected, selected_generation) in bindings {
            let r = rows.get(&id).ok_or_else(corrupt)?;
            let input = r.input()?;
            let eligible =
                controls.get(&r.profile) == Some(&true) && r.valid_until.is_some_and(|v| v > now);
            let evidence = if eligible { r.evidence(&input)? } else { None };
            let selected_candidate = if eligible && selected_generation == Some(r.generation) {
                selected
                    .map(|v| usize::try_from(v).map_err(invalid))
                    .transpose()?
            } else {
                None
            };
            if let Some(index) = selected_candidate {
                let c = evidence
                    .as_ref()
                    .and_then(|e| e.candidates.get(index))
                    .ok_or_else(corrupt)?;
                if !c.acceptable || !candidate_compatible(&input, &c.geography) {
                    return Err(corrupt());
                }
            }
            result.insert(
                subject,
                ResolutionView {
                    work: work(id, r.generation)?,
                    input_revision: u64::try_from(input_revision).map_err(invalid)?,
                    guard: u64::try_from(guard).map_err(invalid)?,
                    status: ResolutionStatus::from_code(&r.status).ok_or_else(corrupt)?,
                    evidence,
                    valid_until: if eligible { r.valid_until } else { None },
                    selected_candidate,
                    last_failure: r.last_failure.clone(),
                    attempts: r.attempts as u32,
                },
            );
        }
        Ok(result)
    }
    async fn inspect(
        &mut self,
        subject: ResolutionSubject,
    ) -> Result<Option<ResolutionView>, ResolutionError> {
        let id: Option<Uuid> = sqlx::query_scalar(
            "SELECT resolution_id FROM geo_resolution_bindings WHERE subject_kind=$1 AND subject_id=$2",
        )
        .bind(subject.kind().as_str())
        .bind(subject.id())
        .fetch_optional(&mut *self.connection)
        .await
        .map_err(sql_error)?;
        let Some(id) = id else {
            return Ok(None);
        };
        let r = self
            .row(ResolutionId::try_from(id).map_err(invalid)?)
            .await?
            .ok_or_else(corrupt)?;
        // Acquiring the resolution lock may wait while the subject changes. Read
        // its guard and selection again under that lock, and hold the binding in
        // the same lock order as selection/requeue/expiry until this read commits.
        let binding: Option<(i64, i64, Option<i32>, Option<i64>)> = sqlx::query_as(
            r#"
            SELECT input_revision,guard,selected_candidate,selected_generation FROM
            geo_resolution_bindings WHERE subject_kind=$1 AND subject_id=$2 AND resolution_id=$3
            FOR SHARE
            "#,
        )
        .bind(subject.kind().as_str())
        .bind(subject.id())
        .bind(id)
        .fetch_optional(&mut *self.connection)
        .await
        .map_err(sql_error)?;
        let Some((input_revision, guard, selected, selected_generation)) = binding else {
            return Ok(None);
        };
        let input = r.input()?;
        let permitted: bool = sqlx::query_scalar(
            "SELECT storage_permitted FROM geo_resolution_provider_controls WHERE profile=$1",
        )
        .bind(&r.profile)
        .fetch_one(&mut *self.connection)
        .await
        .map_err(sql_error)?;
        let now = self.clock().await?;
        let eligible = permitted && r.valid_until.is_some_and(|until| until > now);
        let evidence = if eligible { r.evidence(&input)? } else { None };
        let selected = if selected_generation == Some(r.generation) && eligible {
            selected
                .map(|i| usize::try_from(i).map_err(invalid))
                .transpose()?
        } else {
            None
        };
        if let Some(index) = selected {
            let candidate = evidence
                .as_ref()
                .and_then(|e| e.candidates.get(index))
                .ok_or_else(corrupt)?;
            if !candidate.acceptable || !candidate_compatible(&input, &candidate.geography) {
                return Err(corrupt());
            }
        }
        Ok(Some(ResolutionView {
            work: work(id, r.generation)?,
            input_revision: u64::try_from(input_revision).map_err(invalid)?,
            guard: u64::try_from(guard).map_err(invalid)?,
            status: ResolutionStatus::from_code(&r.status).ok_or_else(corrupt)?,
            evidence,
            valid_until: if eligible { r.valid_until } else { None },
            selected_candidate: selected,
            last_failure: r.last_failure,
            attempts: r.attempts as u32,
        }))
    }
}
