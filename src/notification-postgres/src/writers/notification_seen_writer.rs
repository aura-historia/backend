use application::error::box_error;
use notification_core::notification_id::NotificationId;
use notification_service::ports::notification_seen_writer::{
    NotificationSeenWriteError, NotificationSeenWriter,
};
use sqlx::PgPool;
use user_core::user_id::UserId;

#[derive(Debug, Clone)]
pub struct SqlxNotificationSeenWriter {
    pool: PgPool,
}
impl SqlxNotificationSeenWriter {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait::async_trait]
impl NotificationSeenWriter for SqlxNotificationSeenWriter {
    async fn set_seen(
        &self,
        user_id: UserId,
        notification_id: NotificationId,
        seen: bool,
    ) -> Result<bool, NotificationSeenWriteError> {
        // Identical seen state is a successful no-op for an existing notification; only an
        // actual change advances its freshness.
        sqlx::query_scalar::<_, bool>(
            r#"
            WITH target AS (
                SELECT notification_id FROM notifications
                WHERE user_id = $1 AND notification_id = $2
                FOR UPDATE
            ), changed AS (
                UPDATE notifications n
                SET seen = $3, updated = aura_advance_updated(n.updated)
                FROM target
                WHERE n.user_id = $1 AND n.notification_id = target.notification_id AND n.seen <> $3
            )
            SELECT EXISTS (SELECT 1 FROM target)
            "#,
        )
        .bind(user_id.into_uuid())
        .bind(notification_id.into_uuid())
        .bind(seen)
        .fetch_one(&self.pool)
        .await
        .map_err(|source| NotificationSeenWriteError::UpdateFailed {
            source: box_error(source),
        })
    }
    async fn set_seen_many(
        &self,
        user_id: UserId,
        notification_ids: &[NotificationId],
        seen: bool,
    ) -> Result<u64, NotificationSeenWriteError> {
        if notification_ids.is_empty() {
            return Ok(0);
        }
        let ids = notification_ids
            .iter()
            .copied()
            .map(NotificationId::into_uuid)
            .collect::<Vec<_>>();
        sqlx::query("UPDATE notifications SET seen = $3, updated = aura_advance_updated(updated) WHERE user_id = $1 AND notification_id = ANY($2) AND seen <> $3")
            .bind(user_id.into_uuid()).bind(ids).bind(seen).execute(&self.pool).await
            .map(|result| result.rows_affected()).map_err(|source| NotificationSeenWriteError::UpdateFailed { source: box_error(source) })
    }
    async fn set_seen_all(
        &self,
        user_id: UserId,
        seen: bool,
    ) -> Result<u64, NotificationSeenWriteError> {
        sqlx::query("UPDATE notifications SET seen = $2, updated = aura_advance_updated(updated) WHERE user_id = $1 AND seen <> $2")
            .bind(user_id.into_uuid())
            .bind(seen)
            .execute(&self.pool)
            .await
            .map(|result| result.rows_affected())
            .map_err(|source| NotificationSeenWriteError::UpdateFailed {
                source: box_error(source),
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use test_api::{IntegrationTestService, Postgres, aura_integration_test, get_postgres_client};
    use uuid::Uuid;

    const BUSINESS_SCHEMA: Postgres = Postgres::new("migrations");

    async fn seed(pool: &PgPool) -> (UserId, NotificationId) {
        let user_id = Uuid::now_v7();
        let notification_id = Uuid::now_v7();
        sqlx::query(
            "INSERT INTO users (user_id, email, tier, role) VALUES ($1, $2, 'FREE', 'USER')",
        )
        .bind(user_id)
        .bind(format!("{user_id}@example.test"))
        .execute(pool)
        .await
        .unwrap();
        sqlx::query("INSERT INTO notifications (notification_id, user_id, kind, partnership_application_id, payload_version, payload) VALUES ($1, $2, 'PARTNERSHIP_APPLICATION_APPROVED', $3, 1, $4)")
            .bind(notification_id).bind(user_id).bind(Uuid::now_v7())
            .bind(serde_json::json!({"type": "PARTNERSHIP_APPLICATION", "snapshot": {"party_name": "Test Party", "listing_source_name": "Test Source", "image": null}}))
            .execute(pool).await.unwrap();
        (
            UserId::try_from(user_id).unwrap(),
            NotificationId::try_from(notification_id).unwrap(),
        )
    }

    async fn updated(pool: &PgPool, id: NotificationId) -> time::OffsetDateTime {
        sqlx::query_scalar("SELECT updated FROM notifications WHERE notification_id = $1")
            .bind(id.into_uuid())
            .fetch_one(pool)
            .await
            .unwrap()
    }

    #[aura_integration_test(services = [BUSINESS_SCHEMA])]
    async fn identical_seen_state_succeeds_without_advancing_freshness() {
        let pool = get_postgres_client().await;
        let writer = SqlxNotificationSeenWriter::new(pool.clone());
        let (user, notification) = seed(&pool).await;
        let initial = updated(&pool, notification).await;

        assert!(writer.set_seen(user, notification, false).await.unwrap());
        assert_eq!(initial, updated(&pool, notification).await);
        assert_eq!(
            0,
            writer
                .set_seen_many(user, &[notification], false)
                .await
                .unwrap()
        );
        assert_eq!(0, writer.set_seen_all(user, false).await.unwrap());
        assert_eq!(initial, updated(&pool, notification).await);

        assert!(writer.set_seen(user, notification, true).await.unwrap());
        let seen = updated(&pool, notification).await;
        assert!(seen > initial);
        assert!(writer.set_seen(user, notification, true).await.unwrap());
        assert_eq!(seen, updated(&pool, notification).await);

        assert_eq!(1, writer.set_seen_all(user, false).await.unwrap());
        assert!(updated(&pool, notification).await > seen);
        assert!(
            !writer
                .set_seen(UserId::new(), notification, true)
                .await
                .unwrap()
        );
        assert!(
            !writer
                .set_seen(user, NotificationId::new(), true)
                .await
                .unwrap()
        );
    }
}
