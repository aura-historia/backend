use crate::ports::{
    ListingSourceReadError, PartnershipGrantPolicy, WoocommerceSource, WoocommerceSourceReader,
};
use application::{error::BoxError, operation_context::OperationContext};
use listing_source_core::ListingSourceId;

#[derive(Debug, Clone, PartialEq)]
pub struct GetWoocommerceSourceRequest {
    pub listing_source_id: ListingSourceId,
}
pub type GetWoocommerceSourceResult = WoocommerceSource;
#[derive(Debug, thiserror::Error)]
pub enum GetWoocommerceSourceError {
    #[error("WooCommerce listing source not found")]
    NotFound,
    #[error("partnership grant is required")]
    PartnershipGrantRequired,
    #[error("temporary WooCommerce listing source read failure")]
    TemporarilyUnavailable {
        #[source]
        source: BoxError,
    },
    #[error("invalid WooCommerce listing source read model")]
    InvalidReadModel {
        #[source]
        source: BoxError,
    },
}
#[async_trait::async_trait]
pub trait GetWoocommerceSourceUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        request: GetWoocommerceSourceRequest,
    ) -> Result<GetWoocommerceSourceResult, GetWoocommerceSourceError>;
}
pub struct GetWoocommerceSourceHandler<R, P> {
    reader: R,
    partnership_grants: P,
}
impl<R, P> GetWoocommerceSourceHandler<R, P> {
    pub fn new(reader: R, partnership_grants: P) -> Self {
        Self {
            reader,
            partnership_grants,
        }
    }
}
#[async_trait::async_trait]
impl<R, P> GetWoocommerceSourceUseCase for GetWoocommerceSourceHandler<R, P>
where
    R: WoocommerceSourceReader,
    P: PartnershipGrantPolicy,
{
    #[tracing::instrument(name = "get_woocommerce_source", skip_all, fields(listing_source_id = %request.listing_source_id, principal_type = context.principal.kind(), actor_id = tracing::field::Empty, request_id = %context.request_id, correlation_id = %context.correlation_id))]
    async fn execute(
        &self,
        context: &OperationContext,
        request: GetWoocommerceSourceRequest,
    ) -> Result<GetWoocommerceSourceResult, GetWoocommerceSourceError> {
        tracing::Span::current().record(
            "actor_id",
            tracing::field::display(context.principal.label()),
        );
        let source = self
            .reader
            .find_by_id(request.listing_source_id)
            .await
            .map_err(map_read)?
            .ok_or(GetWoocommerceSourceError::NotFound)?;
        if !self
            .partnership_grants
            .can_access_source(&context.principal, source.listing_source_id)
            .await
            .map_err(map_read)?
        {
            return Err(GetWoocommerceSourceError::PartnershipGrantRequired);
        }
        Ok(source)
    }
}
fn map_read(error: ListingSourceReadError) -> GetWoocommerceSourceError {
    match error {
        ListingSourceReadError::TemporarilyUnavailable { source } => {
            GetWoocommerceSourceError::TemporarilyUnavailable { source }
        }
        ListingSourceReadError::InvalidReadModel { source } => {
            GetWoocommerceSourceError::InvalidReadModel { source }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use application::operation_context::{CorrelationId, Principal, RequestId};
    struct Reader(WoocommerceSource);
    #[async_trait::async_trait]
    impl WoocommerceSourceReader for Reader {
        async fn find_by_id(
            &self,
            _: ListingSourceId,
        ) -> Result<Option<WoocommerceSource>, ListingSourceReadError> {
            Ok(Some(self.0.clone()))
        }
    }
    struct Grants(bool);
    #[async_trait::async_trait]
    impl PartnershipGrantPolicy for Grants {
        async fn can_access_source(
            &self,
            _: &Principal,
            _: ListingSourceId,
        ) -> Result<bool, ListingSourceReadError> {
            Ok(self.0)
        }
    }
    fn source() -> WoocommerceSource {
        WoocommerceSource {
            listing_source_id: ListingSourceId::new(),
            currency: None,
            language: None,
        }
    }
    fn context() -> OperationContext {
        OperationContext {
            principal: Principal::Anonymous,
            request_id: RequestId::new("request"),
            correlation_id: CorrelationId::new("correlation"),
        }
    }
    #[tokio::test]
    async fn should_reject_woocommerce_source_without_partnership_grant() {
        let source = source();
        let handler = GetWoocommerceSourceHandler::new(Reader(source.clone()), Grants(false));
        assert!(matches!(
            handler
                .execute(
                    &context(),
                    GetWoocommerceSourceRequest {
                        listing_source_id: source.listing_source_id
                    }
                )
                .await,
            Err(GetWoocommerceSourceError::PartnershipGrantRequired)
        ));
    }
}
