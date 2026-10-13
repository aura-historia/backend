use crate::ports::*;
use domain_primitives::{change_outcome::ChangeOutcome, event_id::EventId};
use product_listing_core::{
    product_listing::ProductListing, product_listing_event::ProductListingEventPayload,
};

pub(crate) struct PersistedProductListingChange {
    pub version: ProductListingStorageVersion,
    pub outcome: ChangeOutcome,
    pub event_id: Option<EventId>,
    pub changed_fields: Vec<&'static str>,
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum PersistProductListingChangeError {
    #[error(transparent)]
    Repository(#[from] ProductListingRepositoryError),
    #[error(transparent)]
    Event(#[from] ProductListingEventAppendError),
}

/// Uses the caller's transaction. A net no-op performs no state or journal write.
pub(crate) async fn persist_product_listing_change<
    Tx: Send,
    R: ProductListingRepositoryFactory<Tx>,
    E: ProductListingEventAppenderFactory<Tx>,
>(
    tx: &mut Tx,
    products: &R,
    events: &E,
    product: &mut ProductListing,
    version: ProductListingStorageVersion,
) -> Result<PersistedProductListingChange, PersistProductListingChangeError> {
    let Some(payload) = product.take_pending_event_payload() else {
        return Ok(PersistedProductListingChange {
            version,
            outcome: ChangeOutcome::Unchanged,
            event_id: None,
            changed_fields: Vec::new(),
        });
    };
    let mut changed_fields = Vec::new();
    if let ProductListingEventPayload::Changed(changes) = &payload {
        for (field, changed) in [
            ("price", changes.price().is_some()),
            ("priceEstimateMin", changes.price_estimate_min().is_some()),
            ("priceEstimateMax", changes.price_estimate_max().is_some()),
            ("availability", changes.availability().is_some()),
            ("url", changes.url().is_some()),
            ("images", changes.image_count().is_some()),
            ("auction", changes.auction().is_some()),
            ("lifecycle", changes.lifecycle().is_some()),
        ] {
            if changed {
                changed_fields.push(field);
            }
        }
    }
    let event = stamp_product_listing_event(product.id(), time::OffsetDateTime::now_utc(), payload);
    let stored = products
        .in_transaction(tx)
        .update(
            product,
            version,
            event.event_id,
            ProductListingWriteEffects::from(&event.payload),
        )
        .await?;
    events.in_transaction(tx).append(&event).await?;
    Ok(PersistedProductListingChange {
        version: stored.version,
        outcome: ChangeOutcome::Changed,
        event_id: Some(event.event_id),
        changed_fields,
    })
}
