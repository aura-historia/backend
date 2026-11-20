use application::error::{box_error, static_error};
use platform_postgres::SqlxTransaction;
use sqlx::{PgConnection, Row};
use user_service::ports::stripe_subscription_sync::{
    StripeSubscriptionEvent, StripeSubscriptionEventObservation, StripeSubscriptionEventStore,
    StripeSubscriptionEventStoreFactory, StripeSubscriptionSyncError,
};

/// Fingerprint of the event type, subscription and customer an event requests.
const EVENT_REQUEST_FINGERPRINT: &str = "EVENT_REQUEST";
/// Fingerprint of the whole rendered event, written by the retired EventBridge consumer.
const EVENT_SNAPSHOT_FINGERPRINT: &str = "EVENT_SNAPSHOT";

#[derive(Clone, Copy, Default)]
pub struct SqlxStripeSubscriptionEventStoreFactory;

impl StripeSubscriptionEventStoreFactory<SqlxTransaction>
    for SqlxStripeSubscriptionEventStoreFactory
{
    fn in_transaction<'tx>(
        &'tx self,
        tx: &'tx mut SqlxTransaction,
    ) -> impl StripeSubscriptionEventStore + 'tx {
        SqlxStripeSubscriptionEvents {
            connection: tx.connection(),
        }
    }
}

struct SqlxStripeSubscriptionEvents<'tx> {
    connection: &'tx mut PgConnection,
}

fn database_error(source: sqlx::Error) -> StripeSubscriptionSyncError {
    StripeSubscriptionSyncError::Unavailable {
        source: box_error(source),
    }
}

#[async_trait::async_trait]
impl StripeSubscriptionEventStore for SqlxStripeSubscriptionEvents<'_> {
    async fn observe(
        &mut self,
        event: &StripeSubscriptionEvent,
    ) -> Result<StripeSubscriptionEventObservation, StripeSubscriptionSyncError> {
        // Lock event identity first, including when conflicting deliveries name different customers.
        sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 1938))")
            .bind(&event.event_id)
            .execute(&mut *self.connection)
            .await
            .map_err(database_error)?;
        sqlx::query("INSERT INTO stripe_subscription_reconciliations (stripe_customer_id) VALUES ($1) ON CONFLICT DO NOTHING")
            .bind(event.customer_id.as_ref()).execute(&mut *self.connection).await.map_err(database_error)?;
        let revision: i64 = sqlx::query_scalar("SELECT revision FROM stripe_subscription_reconciliations WHERE stripe_customer_id = $1 FOR UPDATE")
            .bind(event.customer_id.as_ref()).fetch_one(&mut *self.connection).await.map_err(database_error)?;
        let receipt = sqlx::query("SELECT stripe_customer_id, stripe_subscription_id, fingerprint, fingerprint_scheme FROM stripe_subscription_event_receipts WHERE stripe_event_id = $1")
            .bind(&event.event_id).fetch_optional(&mut *self.connection).await.map_err(database_error)?;
        if let Some(receipt) = receipt {
            let customer: String = receipt
                .try_get("stripe_customer_id")
                .map_err(database_error)?;
            let subscription: String = receipt
                .try_get("stripe_subscription_id")
                .map_err(database_error)?;
            let fingerprint: Vec<u8> = receipt.try_get("fingerprint").map_err(database_error)?;
            let scheme: String = receipt
                .try_get("fingerprint_scheme")
                .map_err(database_error)?;
            let same_request = match scheme.as_str() {
                EVENT_REQUEST_FINGERPRINT => fingerprint == event.fingerprint,
                // The retired EventBridge consumer hashed the rendered snapshot, which a webhook
                // delivery of the same event cannot reproduce; identity alone must match.
                EVENT_SNAPSHOT_FINGERPRINT => true,
                _ => {
                    return Err(StripeSubscriptionSyncError::InvalidState {
                        source: static_error("unknown Stripe receipt fingerprint scheme"),
                    });
                }
            };
            if customer != event.customer_id.as_ref()
                || subscription != event.subscription_id
                || !same_request
            {
                return Err(StripeSubscriptionSyncError::ReceiptConflict);
            }
            return Ok(StripeSubscriptionEventObservation::Applied);
        }
        let revision = u64::try_from(revision).map_err(|source| {
            StripeSubscriptionSyncError::InvalidState {
                source: box_error(source),
            }
        })?;
        Ok(StripeSubscriptionEventObservation::Pending { revision })
    }

    async fn complete(
        &mut self,
        event: &StripeSubscriptionEvent,
        revision: u64,
    ) -> Result<StripeSubscriptionEventObservation, StripeSubscriptionSyncError> {
        match self.observe(event).await? {
            StripeSubscriptionEventObservation::Applied => {
                return Ok(StripeSubscriptionEventObservation::Applied);
            }
            StripeSubscriptionEventObservation::Pending { revision: current }
                if current != revision =>
            {
                return Err(StripeSubscriptionSyncError::ConcurrentReconciliation);
            }
            StripeSubscriptionEventObservation::Pending { .. } => {}
        }
        let next = revision
            .checked_add(1)
            .and_then(|value| i64::try_from(value).ok())
            .ok_or_else(|| StripeSubscriptionSyncError::InvalidState {
                source: static_error("Stripe reconciliation revision overflow"),
            })?;
        sqlx::query("UPDATE stripe_subscription_reconciliations SET revision = $2 WHERE stripe_customer_id = $1")
            .bind(event.customer_id.as_ref()).bind(next).execute(&mut *self.connection).await.map_err(database_error)?;
        sqlx::query("INSERT INTO stripe_subscription_event_receipts (stripe_event_id, stripe_customer_id, stripe_subscription_id, fingerprint, fingerprint_scheme) VALUES ($1, $2, $3, $4, $5)")
            .bind(&event.event_id).bind(event.customer_id.as_ref()).bind(&event.subscription_id).bind(event.fingerprint.as_slice()).bind(EVENT_REQUEST_FINGERPRINT)
            .execute(&mut *self.connection).await.map_err(database_error)?;
        Ok(StripeSubscriptionEventObservation::Pending { revision })
    }
}
