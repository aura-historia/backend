//! Query-wide public discovery fence, including enrichment, pagination and counts.
use crate::geography::{GeographicQueryGate, with_geographic_eligibility};
use crate::use_cases::{
    SearchProductListingsError, SearchProductListingsRequest, SearchProductListingsResult,
    SearchProductListingsUseCase,
};
use application::operation_context::OperationContext;

#[derive(Debug, Clone, Default, PartialEq)]
pub struct ListingDiscoveryGeography {
    pub business: Option<DiscoveryDimension>,
    pub inventory: Option<DiscoveryDimension>,
}
#[derive(Debug, Clone, PartialEq)]
pub struct DiscoveryDimension {
    pub geography: crate::geography::GeographicDimension,
    pub distance_km: Option<f64>,
}
impl ListingDiscoveryGeography {
    pub fn is_empty(&self) -> bool {
        self.business.is_none() && self.inventory.is_none()
    }
}

pub struct GeographicallyEligibleSearchProductListings<U, G> {
    inner: U,
    gate: G,
}
impl<U, G> GeographicallyEligibleSearchProductListings<U, G> {
    pub fn new(inner: U, gate: G) -> Self {
        Self { inner, gate }
    }
}
#[async_trait::async_trait]
impl<U: SearchProductListingsUseCase, G: GeographicQueryGate> SearchProductListingsUseCase
    for GeographicallyEligibleSearchProductListings<U, G>
{
    async fn execute(
        &self,
        context: &OperationContext,
        request: SearchProductListingsRequest,
    ) -> Result<SearchProductListingsResult, SearchProductListingsError> {
        request
            .search
            .validate_geography()
            .map_err(|_| SearchProductListingsError::InvalidGeographicQuery)?;
        if request.search.distance_query().is_some() && request.sort.is_some() {
            return Err(SearchProductListingsError::InvalidGeographicQuery);
        }
        if !request.search.has_geography() {
            return self.inner.execute(context, request).await;
        }
        let search = request.search.clone();
        with_geographic_eligibility(&self.gate, || async {
            let mut result = self.inner.execute(context, request).await;
            if let Ok(result) = &mut result {
                for item in &mut result.items {
                    for (dimension, query) in [
                        (
                            &mut item.item.geography.business,
                            search.business_geography.as_ref(),
                        ),
                        (
                            &mut item.item.geography.inventory,
                            search.inventory_geography.as_ref(),
                        ),
                    ] {
                        if let (Some(dimension), Some(origin)) =
                            (dimension, query.and_then(|q| q.origin()))
                        {
                            dimension.distance_km = dimension.geography.position.map(|p| {
                                geo::core::discovery::display_distance_km(
                                    geo::core::discovery::distance_km(origin, p.point()),
                                    p.precision(),
                                )
                            });
                        }
                    }
                }
            }
            Ok(result)
        })
        .await
        .map_err(SearchProductListingsError::Geography)?
        .unwrap_or_else(|| Ok(SearchProductListingsResult::default()))
    }
}

/// Periodic saved-filter searches use the same query-wide contract.
pub struct GeographicallyEligibleProductListingSearchReader<R, G> {
    inner: R,
    gate: G,
}
impl<R, G> GeographicallyEligibleProductListingSearchReader<R, G> {
    pub fn new(inner: R, gate: G) -> Self {
        Self { inner, gate }
    }
}
#[async_trait::async_trait]
impl<R: crate::ports::ProductListingSearchReader, G: GeographicQueryGate>
    crate::ports::ProductListingSearchReader
    for GeographicallyEligibleProductListingSearchReader<R, G>
{
    async fn search(
        &self,
        request: &crate::ports::ProductListingSearchReadRequest,
    ) -> Result<
        crate::use_cases::ProductListingSearchReadResult,
        crate::ports::ProductListingSearchReadError,
    > {
        if !request.compiled_search.search.has_geography() {
            return self.inner.search(request).await;
        }
        with_geographic_eligibility(&self.gate, || async {
            Ok(self.inner.search(request).await)
        })
        .await
        .map_err(crate::ports::ProductListingSearchReadError::Geography)?
        .unwrap_or(Err(
            crate::ports::ProductListingSearchReadError::GeographicEligibilityPending,
        ))
    }
    async fn search_hybrid(
        &self,
        request: &crate::ports::ProductListingSearchReadRequest,
        embedding: &[f32],
    ) -> Result<
        crate::use_cases::ProductListingSearchReadResult,
        crate::ports::ProductListingSearchReadError,
    > {
        if !request.compiled_search.search.has_geography() {
            return self.inner.search_hybrid(request, embedding).await;
        }
        with_geographic_eligibility(&self.gate, || async {
            Ok(self.inner.search_hybrid(request, embedding).await)
        })
        .await
        .map_err(crate::ports::ProductListingSearchReadError::Geography)?
        .unwrap_or(Err(
            crate::ports::ProductListingSearchReadError::GeographicEligibilityPending,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geography::{GeographicEligibilityError, GeographicQueryPermit};
    use std::sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    };
    struct Inner(Arc<AtomicUsize>);
    #[async_trait::async_trait]
    impl SearchProductListingsUseCase for Inner {
        async fn execute(
            &self,
            _: &OperationContext,
            _: SearchProductListingsRequest,
        ) -> Result<SearchProductListingsResult, SearchProductListingsError> {
            self.0.fetch_add(1, Ordering::SeqCst);
            let mut result = SearchProductListingsResult {
                total: Some(42),
                ..Default::default()
            };
            result.cursor.search_after = Some(crate::use_cases::ProductListingSearchCursor {
                fx_rate_id: fxrate_core::FxRateId::new(),
                search_after: serde_json::json!([12.3, "private-sort"]),
            });
            Ok(result)
        }
    }
    struct Gate(Mutex<std::collections::VecDeque<Option<GeographicQueryPermit>>>);
    #[async_trait::async_trait]
    impl GeographicQueryGate for Gate {
        async fn permit(
            &self,
        ) -> Result<Option<GeographicQueryPermit>, GeographicEligibilityError> {
            Ok(self.0.lock().unwrap().pop_front().unwrap())
        }
    }
    fn request(geographic: bool) -> SearchProductListingsRequest {
        let mut search =
            product_listing_core::product_listing_search::ProductListingSearch::default();
        if geographic {
            search.business_geography = Some(
                geo::core::discovery::GeographicQuery::new(
                    geo::reference::ReferenceRelease::CURRENT,
                    std::collections::HashSet::from([geo::CountryCode::DEU]),
                    Default::default(),
                    None,
                    None,
                    Default::default(),
                    None,
                )
                .unwrap(),
            );
        }
        SearchProductListingsRequest {
            search,
            sort: None,
            cursor: None,
        }
    }
    fn context() -> OperationContext {
        OperationContext {
            principal: application::operation_context::Principal::Anonymous,
            request_id: "geo-test".into(),
            correlation_id: "geo-test".into(),
        }
    }
    #[tokio::test]
    async fn closed_or_changed_gate_suppresses_complete_result_and_non_geographic_queries_bypass_gate()
     {
        let permit = Some(GeographicQueryPermit {
            epoch: 1,
            ..Default::default()
        });
        for (permits, expected_calls, suppressed) in [
            (vec![None], 0, true),
            (vec![permit, None], 1, true),
            (
                vec![
                    permit,
                    Some(GeographicQueryPermit {
                        epoch: 2,
                        ..Default::default()
                    }),
                ],
                1,
                true,
            ),
            (vec![permit, permit], 1, false),
            (
                vec![
                    Some(GeographicQueryPermit {
                        epoch: 1,
                        eligibility_deadline: Some(time::OffsetDateTime::UNIX_EPOCH),
                    }),
                    permit,
                ],
                1,
                true,
            ),
        ] {
            let calls = Arc::new(AtomicUsize::new(0));
            let handler = GeographicallyEligibleSearchProductListings::new(
                Inner(calls.clone()),
                Gate(Mutex::new(permits.into())),
            );
            let result = handler.execute(&context(), request(true)).await.unwrap();
            assert_eq!(expected_calls, calls.load(Ordering::SeqCst));
            assert_eq!(suppressed, result.total.is_none());
            assert_eq!(suppressed, result.cursor.search_after.is_none());
        }
        let calls = Arc::new(AtomicUsize::new(0));
        let handler = GeographicallyEligibleSearchProductListings::new(
            Inner(calls),
            Gate(Mutex::new(Default::default())),
        );
        assert_eq!(
            Some(42),
            handler
                .execute(&context(), request(false))
                .await
                .unwrap()
                .total
        );
    }
}
