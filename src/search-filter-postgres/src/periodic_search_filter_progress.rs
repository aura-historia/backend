use application::error::box_error;
use platform_postgres::SqlxTransaction;
use search_filter_core::user_search_filter_id::UserSearchFilterId;
use search_filter_service::ports::{
    PeriodicSearchFilterProgress, PeriodicSearchFilterProgressError,
    PeriodicSearchFilterProgressFactory, PeriodicSearchFilterProgressLockOutcome,
    PeriodicSearchFilterProgressWriteOutcome,
};
use time::OffsetDateTime;

#[derive(Debug, Clone, Default)]
pub struct SqlxPeriodicSearchFilterProgressFactory;
struct SqlxPeriodicSearchFilterProgress<'tx> {
    tx: &'tx mut SqlxTransaction,
}

impl PeriodicSearchFilterProgressFactory<SqlxTransaction>
    for SqlxPeriodicSearchFilterProgressFactory
{
    fn in_transaction<'tx>(
        &'tx self,
        tx: &'tx mut SqlxTransaction,
    ) -> impl PeriodicSearchFilterProgress + 'tx {
        SqlxPeriodicSearchFilterProgress { tx }
    }
}

#[async_trait::async_trait]
impl PeriodicSearchFilterProgress for SqlxPeriodicSearchFilterProgress<'_> {
    async fn load_selection(
        &mut self,
        end: OffsetDateTime,
        id: UserSearchFilterId,
    ) -> Result<
        Option<Vec<product_listing_service::ports::ProductListingSearchFilterMatchSourceRef>>,
        PeriodicSearchFilterProgressError,
    > {
        let (listings, events) = sqlx::query_as::<_, (Option<Vec<uuid::Uuid>>, Option<Vec<uuid::Uuid>>)>("SELECT selected_listing_ids, selected_event_ids FROM search_filter_matching_run_filters WHERE window_end = $1 AND user_search_filter_id = $2")
            .bind(end).bind(id.into_uuid()).fetch_one(self.tx.connection()).await.map_err(run_error)?;
        match (listings, events) {
            (None, None) => Ok(None),
            (Some(listings), Some(events))
                if listings.len() == events.len() && listings.len() <= 50 =>
            {
                let mut seen = std::collections::HashSet::new();
                let refs = listings.into_iter().zip(events).map(|(listing, event)| {
                    if !seen.insert(listing) { return Err(invalid_selection()); }
                    Ok(product_listing_service::ports::ProductListingSearchFilterMatchSourceRef {
                        product_listing_id: product_listing_core::product_listing_id::ProductListingId::try_from(listing).map_err(|source| PeriodicSearchFilterProgressError::PersistenceFailed { source: box_error(source) })?,
                        event_id: domain_primitives::event_id::EventId::try_from(event).map_err(|source| PeriodicSearchFilterProgressError::PersistenceFailed { source: box_error(source) })?,
                    })
                }).collect::<Result<Vec<_>, _>>()?;
                Ok(Some(refs))
            }
            _ => Err(invalid_selection()),
        }
    }

    async fn freeze_selection(
        &mut self,
        end: OffsetDateTime,
        id: UserSearchFilterId,
        selected: &[product_listing_service::ports::ProductListingSearchFilterMatchSourceRef],
    ) -> Result<
        Vec<product_listing_service::ports::ProductListingSearchFilterMatchSourceRef>,
        PeriodicSearchFilterProgressError,
    > {
        let listings = selected
            .iter()
            .map(|value| value.product_listing_id.into_uuid())
            .collect::<Vec<_>>();
        let events = selected
            .iter()
            .map(|value| value.event_id.into_uuid())
            .collect::<Vec<_>>();
        sqlx::query("UPDATE search_filter_matching_run_filters SET selected_listing_ids = $3, selected_event_ids = $4 WHERE window_end = $1 AND user_search_filter_id = $2 AND selected_listing_ids IS NULL")
            .bind(end).bind(id.into_uuid()).bind(listings).bind(events).execute(self.tx.connection()).await.map_err(run_error)?;
        self.load_selection(end, id)
            .await?
            .ok_or_else(invalid_selection)
    }

    async fn begin_run(
        &mut self,
        proposed: OffsetDateTime,
    ) -> Result<OffsetDateTime, PeriodicSearchFilterProgressError> {
        sqlx::query("UPDATE search_filter_matching_runs SET status = 'INTERRUPTED' WHERE status = 'RUNNING' AND recovery_deadline <= now()")
            .execute(self.tx.connection()).await.map_err(run_error)?;
        if let Some(window_end) = sqlx::query_scalar("SELECT window_end FROM search_filter_matching_runs WHERE status = 'RUNNING' FOR UPDATE")
            .fetch_optional(self.tx.connection()).await.map_err(run_error)? {
            return Ok(window_end);
        }
        let inserted = sqlx::query_scalar::<_, OffsetDateTime>("INSERT INTO search_filter_matching_runs(window_end) VALUES ($1) ON CONFLICT DO NOTHING RETURNING window_end")
            .bind(proposed).fetch_optional(self.tx.connection()).await.map_err(run_error)?;
        if let Some(window_end) = inserted {
            sqlx::query(r#"INSERT INTO search_filter_matching_run_filters(window_end, user_id, user_search_filter_id, snapshot)
                SELECT $1, f.user_id, f.user_search_filter_id, to_jsonb(f) || jsonb_build_object('matched_through', COALESCE(p.matched_through, f.created))
                FROM search_filters f LEFT JOIN search_filter_periodic_match_state p USING(user_search_filter_id)
                WHERE f.state = 'ACTIVE' AND f.created <= $1 AND COALESCE(p.matched_through, f.created) < $1"#)
                .bind(window_end).execute(self.tx.connection()).await.map_err(run_error)?;
            sqlx::query("INSERT INTO saved_search_digest_groups(window_end, user_id) SELECT DISTINCT window_end, user_id FROM search_filter_matching_run_filters WHERE window_end = $1")
                .bind(window_end).execute(self.tx.connection()).await.map_err(run_error)?;
            return Ok(window_end);
        }
        sqlx::query_scalar(
            "SELECT window_end FROM search_filter_matching_runs WHERE window_end = $1",
        )
        .bind(proposed)
        .fetch_one(self.tx.connection())
        .await
        .map_err(run_error)
    }

    async fn record_filter_attempt(
        &mut self,
        window_end: OffsetDateTime,
        id: UserSearchFilterId,
        completed: bool,
    ) -> Result<(), PeriodicSearchFilterProgressError> {
        sqlx::query("UPDATE search_filter_matching_run_filters SET terminal = terminal OR $3, failed_attempts = failed_attempts + CASE WHEN $3 THEN 0 ELSE 1 END WHERE window_end = $1 AND user_search_filter_id = $2")
            .bind(window_end).bind(id.into_uuid()).bind(completed).execute(self.tx.connection()).await.map_err(run_error)?;
        Ok(())
    }

    async fn finish_run(
        &mut self,
        window_end: OffsetDateTime,
    ) -> Result<(), PeriodicSearchFilterProgressError> {
        sqlx::query("UPDATE search_filter_matching_runs r SET status = 'COMPLETED' WHERE window_end = $1 AND status = 'RUNNING' AND NOT EXISTS (SELECT 1 FROM search_filter_matching_run_filters f WHERE f.window_end = r.window_end AND NOT terminal)")
            .bind(window_end).execute(self.tx.connection()).await.map_err(run_error)?;
        Ok(())
    }

    async fn lock_and_read(
        &mut self,
        search_filter_id: UserSearchFilterId,
        expected_version: i64,
        created: OffsetDateTime,
        window_end: OffsetDateTime,
    ) -> Result<PeriodicSearchFilterProgressLockOutcome, PeriodicSearchFilterProgressError> {
        let id = search_filter_id.into_uuid();
        let version = sqlx::query_scalar::<_, i64>(
            "SELECT version FROM search_filters WHERE user_search_filter_id = $1 AND state = 'ACTIVE' FOR UPDATE",
        )
        .bind(id)
        .fetch_optional(self.tx.connection())
        .await
        .map_err(|source| PeriodicSearchFilterProgressError::PersistenceFailed {
            source: box_error(source),
        })?;
        if version != Some(expected_version) {
            return Ok(PeriodicSearchFilterProgressLockOutcome::ChangedOrInactive);
        }
        let matched_through = sqlx::query_scalar::<_, OffsetDateTime>("SELECT matched_through FROM search_filter_periodic_match_state WHERE user_search_filter_id = $1 FOR UPDATE")
            .bind(id).fetch_optional(self.tx.connection()).await
            .map_err(|source| PeriodicSearchFilterProgressError::PersistenceFailed { source: box_error(source) })?
            .unwrap_or(created);
        let _ = window_end;
        Ok(PeriodicSearchFilterProgressLockOutcome::Current { matched_through })
    }

    async fn compare_and_set(
        &mut self,
        search_filter_id: UserSearchFilterId,
        expected: OffsetDateTime,
        matched_through: OffsetDateTime,
    ) -> Result<PeriodicSearchFilterProgressWriteOutcome, PeriodicSearchFilterProgressError> {
        if matched_through <= expected {
            return Ok(PeriodicSearchFilterProgressWriteOutcome::AlreadyCovered);
        }
        let id = search_filter_id.into_uuid();
        let changed = sqlx::query_scalar::<_, uuid::Uuid>(
            r#"
            INSERT INTO search_filter_periodic_match_state (user_search_filter_id, matched_through)
            SELECT $1, $3
            WHERE $2 < $3
            ON CONFLICT (user_search_filter_id) DO UPDATE
              SET matched_through = EXCLUDED.matched_through, updated = now()
              WHERE search_filter_periodic_match_state.matched_through = $2
                AND search_filter_periodic_match_state.matched_through < EXCLUDED.matched_through
            RETURNING user_search_filter_id
        "#,
        )
        .bind(id)
        .bind(expected)
        .bind(matched_through)
        .fetch_optional(self.tx.connection())
        .await
        .map_err(
            |source| PeriodicSearchFilterProgressError::PersistenceFailed {
                source: box_error(source),
            },
        )?;
        Ok(if changed.is_some() {
            PeriodicSearchFilterProgressWriteOutcome::Advanced
        } else {
            PeriodicSearchFilterProgressWriteOutcome::Superseded
        })
    }
}

fn run_error(source: sqlx::Error) -> PeriodicSearchFilterProgressError {
    PeriodicSearchFilterProgressError::PersistenceFailed {
        source: box_error(source),
    }
}
fn invalid_selection() -> PeriodicSearchFilterProgressError {
    PeriodicSearchFilterProgressError::PersistenceFailed {
        source: box_error(std::io::Error::other(
            "invalid periodic matching candidate selection",
        )),
    }
}
