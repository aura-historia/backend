//! Optional deterministic crawler assertions use the ordinary delivery command and import fences.
use crate::{
    ports::{
        ProductListingRawRevisionId, ProductListingStorageVersion,
        product_listing_delivery::CrawlerShippingSourceReader,
    },
    use_cases::{
        ChangeProductListingDeliveryCommand, ChangeProductListingDeliveryUseCase,
        ProductListingDeliveryError, ProductListingDeliveryPatch,
    },
};
use application::{
    operation_context::{CorrelationId, OperationContext, Principal, RequestId},
    patch_field::PatchField,
};
use geo::reference::ReferenceRelease;
use product_listing_core::{delivery::DeliveryIntegration, product_listing_id::ProductListingKey};
use product_listing_normalization::shipping::ItemShippingAssertions;
use shipping_core::{
    coverage::{CoverageCompleteness, DeliveryArea, ShippingCoverage, ShippingZone},
    inputs::Applicability,
    pricing::ShippingRate,
    selection::ComponentOverride,
    terms::{
        ShippingEligibility, ShippingPolicy, ShippingService, ShippingServiceCode,
        ShippingServiceTerms,
    },
    time::LeadTimes,
};
use shipping_service::imports::{ShippingImportWrite, ShippingObservation};
use time::OffsetDateTime;

#[derive(Debug, Clone)]
pub struct CrawlerShippingObservation {
    pub raw_revision_id: ProductListingRawRevisionId,
    pub product_key: ProductListingKey,
    /// Captured by the catalog transaction, never refetched to retry old content.
    pub expected_version: ProductListingStorageVersion,
    pub acquired_at: Option<OffsetDateTime>,
    pub assertions: ItemShippingAssertions,
}

#[async_trait::async_trait]
pub trait ApplyCrawlerShippingUseCase: Send + Sync {
    async fn execute(
        &self,
        observation: CrawlerShippingObservation,
    ) -> Result<&'static str, ProductListingDeliveryError>;
}

pub struct ApplyCrawlerShippingHandler<R, W> {
    sources: R,
    delivery: W,
}
impl<R, W> ApplyCrawlerShippingHandler<R, W> {
    pub fn new(sources: R, delivery: W) -> Self {
        Self { sources, delivery }
    }
}

#[async_trait::async_trait]
impl<R: CrawlerShippingSourceReader, W: ChangeProductListingDeliveryUseCase>
    ApplyCrawlerShippingUseCase for ApplyCrawlerShippingHandler<R, W>
{
    async fn execute(
        &self,
        observation: CrawlerShippingObservation,
    ) -> Result<&'static str, ProductListingDeliveryError> {
        if !self
            .sources
            .is_non_native(observation.product_key.listing_source_id)
            .await?
        {
            return Ok("SKIPPED_NATIVE_SOURCE");
        }
        let Some(command) = command(&observation, OffsetDateTime::now_utc())? else {
            return Ok("NO_APPLICABLE_ASSERTION");
        };
        let context = OperationContext {
            principal: Principal::Service("crawler".to_owned()),
            request_id: RequestId::new(format!("crawler-shipping:{}", observation.raw_revision_id)),
            correlation_id: CorrelationId::new(
                observation.product_key.listing_source_id.to_string(),
            ),
        };
        // One bounded retry for a lost response or transient storage failure. Never refetch a
        // revision, mint a key, or advance the observation time while retrying this acquisition.
        match self.delivery.execute(&context, command.clone()).await {
            Ok(_) => Ok("APPLIED"),
            Err(
                ProductListingDeliveryError::OperationFailed { .. }
                | ProductListingDeliveryError::Transaction(_)
                | ProductListingDeliveryError::Import(
                    shipping_service::ShippingError::PersistenceUnavailable { .. }
                    | shipping_service::ShippingError::PersistenceFailed { .. }
                    | shipping_service::ShippingError::Transaction(_),
                ),
            ) => self
                .delivery
                .execute(&context, command)
                .await
                .map(|_| "APPLIED"),
            Err(error) => Err(error),
        }
    }
}

fn command(
    observation: &CrawlerShippingObservation,
    now: OffsetDateTime,
) -> Result<Option<ChangeProductListingDeliveryCommand>, ProductListingDeliveryError> {
    let Some(acquired_at) = observation.acquired_at else {
        return Ok(None);
    };
    if acquired_at > now {
        return Ok(None);
    }
    let sequence = u64::try_from(acquired_at.unix_timestamp_nanos() / 1000)
        .ok()
        .filter(|v| *v > 0 && *v <= i64::MAX as u64);
    let Some(sequence) = sequence else {
        return Ok(None);
    };
    let a = &observation.assertions;
    let policy = if a.prohibited {
        ShippingPolicy::Prohibited
    } else if a.worldwide {
        let service = ShippingService::new(
            ShippingServiceCode::new("ITEM_SHIPPING".to_owned()).map_err(invalid)?,
            ShippingServiceTerms {
                eligibility: a.offered.then_some(ShippingEligibility::Offered),
                rate: a
                    .price
                    .map(|p| ShippingRate::new(p, None, None, Default::default())),
                lead_times: LeadTimes {
                    handling: a.handling,
                    ..Default::default()
                },
                ..Default::default()
            },
        );
        ShippingPolicy::coverage(
            ShippingCoverage::new(
                ReferenceRelease::CURRENT,
                CoverageCompleteness::Partial,
                Applicability::default(),
                vec![],
                vec![
                    ShippingZone::new(
                        vec![DeliveryArea::Anywhere],
                        Applicability::default(),
                        vec![service],
                    )
                    .map_err(invalid)?,
                ],
                None,
            )
            .map_err(invalid)?,
        )
    } else {
        // A dispatch duration alone cannot supply a destination or identify an inherited service.
        return Ok(None);
    };
    Ok(Some(ChangeProductListingDeliveryCommand {
        product_key: observation.product_key.clone(),
        expected_version: observation.expected_version,
        import: Some(ShippingImportWrite {
            integration: DeliveryIntegration::Source,
            sequence,
            acquired_at,
            request_key: format!("crawler-shipping:{}", observation.raw_revision_id),
            object_key: None,
            observation: Some(ShippingObservation { acquired_at }),
        }),
        delivery: PatchField::Set(ProductListingDeliveryPatch {
            integration: DeliveryIntegration::Source,
            shipping: PatchField::Set(ComponentOverride::Replace(policy)),
            ..Default::default()
        }),
    }))
}

fn invalid(source: shipping_core::InvalidShippingValue) -> ProductListingDeliveryError {
    ProductListingDeliveryError::InvalidInput {
        source: Box::new(source),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::use_cases::ChangeProductListingDeliveryResult;
    use std::sync::{Arc, Mutex};
    use time::Duration;

    fn observation(text: &str) -> CrawlerShippingObservation {
        let product_listing_normalization::shipping::ShippingQuickCheck::Resolved(assertions) =
            product_listing_normalization::shipping::quick_check_shipping_with_context(
                &[text],
                &[],
            )
        else {
            panic!("supported fixture");
        };
        CrawlerShippingObservation {
            raw_revision_id: ProductListingRawRevisionId::new(),
            product_key: ProductListingKey::new(
                listing_source_core::ListingSourceId::new(),
                "item-1".try_into().unwrap(),
            ),
            expected_version: ProductListingStorageVersion::INITIAL,
            acquired_at: Some(OffsetDateTime::now_utc() - Duration::minutes(10)),
            assertions,
        }
    }

    struct Source(bool);
    #[async_trait::async_trait]
    impl CrawlerShippingSourceReader for Source {
        async fn is_non_native(
            &self,
            _: listing_source_core::ListingSourceId,
        ) -> Result<bool, ProductListingDeliveryError> {
            Ok(self.0)
        }
    }
    struct Delivery(Arc<Mutex<Vec<ChangeProductListingDeliveryCommand>>>);
    #[async_trait::async_trait]
    impl ChangeProductListingDeliveryUseCase for Delivery {
        async fn execute(
            &self,
            _: &OperationContext,
            command: ChangeProductListingDeliveryCommand,
        ) -> Result<ChangeProductListingDeliveryResult, ProductListingDeliveryError> {
            self.0.lock().unwrap().push(command);
            Err(ProductListingDeliveryError::Transaction(Box::new(
                std::io::Error::other("lost response"),
            )))
        }
    }

    #[tokio::test]
    async fn retries_keep_the_exact_original_request_and_revision() {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let observation = observation("This item is collection only");
        assert!(
            ApplyCrawlerShippingHandler::new(Source(true), Delivery(calls.clone()))
                .execute(observation.clone())
                .await
                .is_err()
        );
        let calls = calls.lock().unwrap();
        assert_eq!(2, calls.len());
        assert_eq!(calls[0], calls[1]);
        assert_eq!(observation.expected_version, calls[0].expected_version);
        let import = calls[0].import.as_ref().unwrap();
        assert_eq!(observation.acquired_at, Some(import.acquired_at));
        assert_eq!(
            import.acquired_at,
            import.observation.as_ref().unwrap().acquired_at
        );
        let PatchField::Set(patch) = &calls[0].delivery else {
            panic!("patch");
        };
        assert!(matches!(
            patch.shipping,
            PatchField::Set(ComponentOverride::Replace(ShippingPolicy::Prohibited))
        ));
        assert!(matches!(patch.collection, PatchField::Unchanged));
        assert!(matches!(patch.selection, PatchField::Unchanged));
        assert!(matches!(patch.rate_inputs, PatchField::Unchanged));
    }

    #[tokio::test]
    async fn native_missing_acquisition_future_and_unscoped_observations_do_not_write() {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let o = observation("This item is collection only");
        assert_eq!(
            "SKIPPED_NATIVE_SOURCE",
            ApplyCrawlerShippingHandler::new(Source(false), Delivery(calls.clone()))
                .execute(o.clone())
                .await
                .unwrap()
        );
        let handler = ApplyCrawlerShippingHandler::new(Source(true), Delivery(calls.clone()));
        for acquired_at in [None, Some(OffsetDateTime::now_utc() + Duration::days(1))] {
            let mut old = o.clone();
            old.acquired_at = acquired_at;
            assert_eq!(
                "NO_APPLICABLE_ASSERTION",
                handler.execute(old).await.unwrap()
            );
        }
        assert_eq!(
            "NO_APPLICABLE_ASSERTION",
            handler
                .execute(observation("Dispatch in 10–15 business days"))
                .await
                .unwrap()
        );
        assert!(calls.lock().unwrap().is_empty());
    }

    #[test]
    fn scoped_pricing_keeps_eligibility_absent_and_dispatch_in_handling() {
        let o = observation(
            "Shipping costs for this item worldwide on request. Dispatch in 10–15 business days.",
        );
        let command = command(&o, OffsetDateTime::now_utc()).unwrap().unwrap();
        let PatchField::Set(patch) = command.delivery else {
            panic!("patch");
        };
        let PatchField::Set(ComponentOverride::Replace(ShippingPolicy::Coverage(policy))) =
            patch.shipping
        else {
            panic!("coverage");
        };
        assert_eq!(None, policy.dispatch_origin());
        let service = &policy.zones()[0].services()[0];
        assert_eq!(None, service.eligibility());
        assert_eq!(None, service.rate().unwrap().basis());
        assert_eq!(None, service.rate().unwrap().price().currency());
        assert_eq!(10, service.lead_times().handling.unwrap().minimum());
        assert_eq!(None, service.lead_times().transit);
        assert_eq!(None, service.lead_times().total);
    }

    #[test]
    fn reprocessing_preserves_acquisition_without_an_age_cutoff() {
        let o = observation("This item is collection only");
        let acquired_at = o.acquired_at.unwrap();
        let first = command(&o, acquired_at + Duration::hours(1))
            .unwrap()
            .unwrap();
        let reprocessed = command(&o, acquired_at + Duration::days(365))
            .unwrap()
            .unwrap();
        assert_eq!(first, reprocessed);
        assert_eq!(
            acquired_at,
            reprocessed.import.unwrap().observation.unwrap().acquired_at
        );
    }
}
