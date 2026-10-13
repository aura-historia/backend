use application::error::box_error;
use notification_service::saved_search_digest::{
    SavedSearchDigestError, SavedSearchDigestMaintenanceReport, SavedSearchDigestStore,
    SavedSearchDigestStoreFactory,
};
use platform_postgres::SqlxTransaction;
use sqlx::PgConnection;
use time::OffsetDateTime;
use uuid::Uuid;

#[derive(Clone, Copy, Default)]
pub struct SqlxSavedSearchDigestStoreFactory;
struct Store<'a>(&'a mut PgConnection);
impl SavedSearchDigestStoreFactory<SqlxTransaction> for SqlxSavedSearchDigestStoreFactory {
    fn in_transaction<'a>(
        &'a self,
        tx: &'a mut SqlxTransaction,
    ) -> impl SavedSearchDigestStore + 'a {
        Store(tx.connection())
    }
}

#[async_trait::async_trait]
impl SavedSearchDigestStore for Store<'_> {
    async fn finalize_next(
        &mut self,
    ) -> Result<Option<SavedSearchDigestMaintenanceReport>, SavedSearchDigestError> {
        sqlx::query("UPDATE search_filter_matching_runs SET status = 'INTERRUPTED' WHERE status = 'RUNNING' AND recovery_deadline <= now()")
            .execute(&mut *self.0).await.map_err(error)?;
        let group = sqlx::query_as::<_, (OffsetDateTime, Uuid, f64)>(r#"
            SELECT g.window_end, g.user_id, EXTRACT(EPOCH FROM now() - r.created)::float8
            FROM saved_search_digest_groups g JOIN search_filter_matching_runs r USING(window_end)
            WHERE g.finalized_at IS NULL
              AND (r.status <> 'RUNNING' OR NOT EXISTS (
                SELECT 1 FROM search_filter_matching_run_filters f WHERE f.window_end = g.window_end AND f.user_id = g.user_id AND NOT f.terminal))
              AND NOT EXISTS (SELECT 1 FROM search_filter_matches m WHERE m.matching_run_end = g.window_end AND m.user_id = g.user_id AND NOT m.notification_selection_complete)
            ORDER BY g.window_end, g.user_id FOR UPDATE OF g SKIP LOCKED LIMIT 1
        "#).fetch_optional(&mut *self.0).await.map_err(error)?;
        let Some((end, user, age_seconds)) = group else {
            return Ok(None);
        };
        let user_id = user_core::user_id::UserId::try_from(user).map_err(|source| {
            SavedSearchDigestError {
                source: box_error(source),
            }
        })?;
        // Carry forward only late members of finalized groups. An older group
        // still awaiting selection owns its members until it can finalize.
        let members = sqlx::query_scalar::<_, Uuid>(r#"
            SELECT m.notification_id FROM saved_search_digest_members m
            JOIN saved_search_digest_groups g ON g.window_end = m.matching_run_end AND g.user_id = m.user_id
            WHERE m.user_id = $1 AND m.digest_id IS NULL
              AND (m.matching_run_end = $2 OR (m.matching_run_end < $2 AND g.finalized_at IS NOT NULL))
            ORDER BY m.matching_run_end, m.created, m.origin_event_id, m.user_search_filter_id, m.product_listing_id
            FOR UPDATE OF m
        "#)
            .bind(user).bind(end).fetch_all(&mut *self.0).await.map_err(error)?;
        if !members.is_empty() {
            let digest_id = notification_core::saved_search_digest_id::SavedSearchDigestId::new();
            sqlx::query("INSERT INTO saved_search_digests(digest_id, window_end, user_id) VALUES ($1, $2, $3)")
                .bind(digest_id.into_uuid()).bind(end).bind(user).execute(&mut *self.0).await.map_err(error)?;
            sqlx::query("UPDATE saved_search_digest_members SET digest_id = $1 WHERE notification_id = ANY($2) AND digest_id IS NULL")
                .bind(digest_id.into_uuid()).bind(&members).execute(&mut *self.0).await.map_err(error)?;
            sqlx::query("INSERT INTO notification_deliveries(notification_delivery_id, digest_id, channel, target_key) VALUES ($1, $2, 'EMAIL', 'PRIMARY')")
                .bind(Uuid::now_v7()).bind(digest_id.into_uuid()).execute(&mut *self.0).await.map_err(error)?;
            tracing::info!(%digest_id, run_end = %end, user_id = %user_id, eligible_matches = members.len(), age_seconds, "saved_search.digest.finalized");
        }
        sqlx::query("UPDATE saved_search_digest_groups SET finalized_at = now() WHERE window_end = $1 AND user_id = $2")
            .bind(end).bind(user).execute(&mut *self.0).await.map_err(error)?;
        Ok(Some(SavedSearchDigestMaintenanceReport {
            groups_finalized: 1,
            digests_created: usize::from(!members.is_empty()),
            matches_enrolled: members.len(),
        }))
    }
}
fn error(source: sqlx::Error) -> SavedSearchDigestError {
    SavedSearchDigestError {
        source: box_error(source),
    }
}

#[derive(sqlx::FromRow)]
struct DigestTarget {
    digest_id: Uuid,
    user_id: Uuid,
    window_end: OffsetDateTime,
    payload_version: i16,
    channel: String,
    target_key: String,
    language: Option<String>,
    show_unassessed_or_sensitive_content: bool,
}
#[derive(sqlx::FromRow)]
struct Member {
    #[sqlx(flatten)]
    notification: crate::mapping::NotificationRow,
    decision: Option<String>,
    category: Option<String>,
    member_user_id: Uuid,
    member_filter_id: Uuid,
    member_listing_id: Uuid,
    member_event_id: Uuid,
    member_notification_id: Uuid,
}

pub(crate) async fn load_delivery_source(
    connection: &mut PgConnection,
    delivery_id: notification_core::notification_delivery_id::NotificationDeliveryId,
) -> Result<
    Option<
        notification_service::ports::notification_delivery_repository::NotificationDeliverySource,
    >,
    notification_service::ports::notification_delivery_repository::NotificationDeliveryError,
> {
    use notification_core::{
        notification::{Notification, NotificationContent},
        notification_delivery::NotificationDeliveryTargetKey,
    };
    use notification_service::{
        ports::notification_delivery_repository::{
            NotificationDeliveryError, NotificationDeliverySource,
        },
        presentation::NotificationPresentationPreferences,
        saved_search_digest::SavedSearchEmailDigest,
    };
    use product_listing_core::content_policy::{ContentPolicyDecision, SensitiveContentCategory};
    let target = sqlx::query_as::<_, DigestTarget>(r#"SELECT g.digest_id, g.user_id, g.window_end, g.payload_version, d.channel, d.target_key, u.language, u.show_unassessed_or_sensitive_content
        FROM notification_deliveries d JOIN saved_search_digests g USING(digest_id) JOIN users u ON u.user_id = g.user_id
        WHERE d.notification_delivery_id = $1 AND NOT u.suspended"#)
        .bind(delivery_id.into_uuid()).fetch_optional(&mut *connection).await.map_err(delivery_error)?;
    let Some(target) = target else {
        return Ok(None);
    };
    if target.payload_version != 1 {
        return Err(invalid("unsupported saved-search digest payload version"));
    }
    let digest_id =
        notification_core::saved_search_digest_id::SavedSearchDigestId::try_from(target.digest_id)
            .map_err(|source| NotificationDeliveryError::InvalidPersistedState {
                source: box_error(source),
            })?;
    let user_id = user_core::user_id::UserId::try_from(target.user_id).map_err(|source| {
        NotificationDeliveryError::InvalidPersistedState {
            source: box_error(source),
        }
    })?;
    let rows = sqlx::query_as::<_, Member>(r#"
        SELECT n.notification_id, n.user_id, n.kind, n.origin_event_id, n.product_listing_id, n.user_search_filter_id,
               n.partnership_application_id, n.payload_version, n.payload, n.seen, n.created, n.updated,
               a.decision, a.category, m.user_id AS member_user_id, m.user_search_filter_id AS member_filter_id,
               m.product_listing_id AS member_listing_id, m.origin_event_id AS member_event_id,
               m.notification_id AS member_notification_id
        FROM saved_search_digest_members m
        CROSS JOIN LATERAL jsonb_populate_record(NULL::notifications, m.snapshot) n
        JOIN search_filters f ON f.user_search_filter_id = m.user_search_filter_id AND f.user_id = m.user_id
        JOIN product_listings p ON p.product_listing_id = m.product_listing_id
        LEFT JOIN product_listing_content_assessments a ON a.product_listing_id = p.product_listing_id AND a.source_event_id = p.content_source_event_id
        WHERE m.digest_id = $1 AND f.state = 'ACTIVE' AND f.notifications AND p.lifecycle = 'ACTIVE' AND p.current_event_id = m.origin_event_id
        ORDER BY m.created, m.origin_event_id, m.user_search_filter_id, m.product_listing_id
    "#).bind(target.digest_id).fetch_all(&mut *connection).await.map_err(delivery_error)?;
    let mut members = Vec::with_capacity(rows.len());
    for row in rows {
        if row.notification.notification_id != row.member_notification_id
            || row.notification.user_id != target.user_id
            || row.notification.user_id != row.member_user_id
            || row.notification.user_search_filter_id != Some(row.member_filter_id)
            || row.notification.product_listing_id != Some(row.member_listing_id)
            || row.notification.origin_event_id != Some(row.member_event_id)
        {
            return Err(invalid("saved-search digest snapshot identity mismatch"));
        }
        let policy = match (row.decision.as_deref(), row.category.as_deref()) {
            (None, None) => None,
            (Some("ALLOWED"), None) => Some(ContentPolicyDecision::Allowed),
            (Some("REQUIRES_CONSENT"), Some("NAZI_GERMANY")) => Some(
                ContentPolicyDecision::RequiresConsent(SensitiveContentCategory::NaziGermany),
            ),
            _ => return Err(invalid("invalid current saved-search content assessment")),
        };
        let notification = Notification::try_from(row.notification).map_err(|source| {
            NotificationDeliveryError::InvalidPersistedState {
                source: crate::mapping::mapping_error(source),
            }
        })?;
        let mut content = notification.content().clone();
        match &mut content {
            NotificationContent::SearchFilter { snapshot, .. } => snapshot.content_policy = policy,
            _ => {
                return Err(invalid(
                    "saved-search digest contains an unrelated notification",
                ));
            }
        }
        members.push(content);
    }
    let Some(content) = members.first().cloned() else {
        tracing::info!(digest_id = %digest_id, run_end = %target.window_end, "saved_search.digest.suppressed_current_eligibility");
        return Ok(None);
    };
    let language = match target.language.as_deref() {
        Some(code) => localization::Language::from_code(code)
            .ok_or_else(|| invalid("unknown saved-search recipient language"))?,
        None => localization::Language::En,
    };
    Ok(Some(NotificationDeliverySource {
        notification_delivery_id: delivery_id,
        source_id: notification_service::ports::notification_delivery_repository::NotificationDeliverySourceId::SavedSearchDigest(digest_id),
        user_id,
        channel: crate::delivery_mapping::channel_from_persisted(&target.channel).map_err(|source| {
            NotificationDeliveryError::InvalidPersistedState { source: box_error(source) }
        })?,
        target_key: NotificationDeliveryTargetKey::try_from(target.target_key).map_err(|source| {
            NotificationDeliveryError::InvalidPersistedState { source: box_error(source) }
        })?,
        content,
        saved_search_digest: Some(SavedSearchEmailDigest {
            digest_id,
            window_end: target.window_end,
            members,
        }),
        presentation_preferences: NotificationPresentationPreferences {
            language,
            show_unassessed_or_sensitive_content: target.show_unassessed_or_sensitive_content,
        },
    }))
}
fn delivery_error(
    source: sqlx::Error,
) -> notification_service::ports::notification_delivery_repository::NotificationDeliveryError {
    notification_service::ports::notification_delivery_repository::NotificationDeliveryError::OperationFailed { source: box_error(source) }
}
fn invalid(
    message: &'static str,
) -> notification_service::ports::notification_delivery_repository::NotificationDeliveryError {
    notification_service::ports::notification_delivery_repository::NotificationDeliveryError::InvalidPersistedState { source: box_error(std::io::Error::other(message)) }
}
