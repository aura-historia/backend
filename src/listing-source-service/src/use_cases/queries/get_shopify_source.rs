use crate::ports::{
    ListingSourceReadError, PartnershipGrantPolicy, ShopifySource, ShopifySourceReader,
};
use application::{error::BoxError, operation_context::OperationContext};
use listing_source_core::Domain;

#[derive(Debug, Clone, PartialEq)]
pub struct GetShopifySourceRequest {
    pub domain: Domain,
}
pub type GetShopifySourceResult = ShopifySource;
#[derive(Debug, thiserror::Error)]
pub enum GetShopifySourceError {
    #[error("Shopify listing source not found")]
    NotFound,
    #[error("partnership grant is required")]
    PartnershipGrantRequired,
    #[error("temporary Shopify listing source read failure")]
    TemporarilyUnavailable {
        #[source]
        source: BoxError,
    },
    #[error("invalid Shopify listing source read model")]
    InvalidReadModel {
        #[source]
        source: BoxError,
    },
}
#[async_trait::async_trait]
pub trait GetShopifySourceUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        request: GetShopifySourceRequest,
    ) -> Result<GetShopifySourceResult, GetShopifySourceError>;
}
pub struct GetShopifySourceHandler<R, P> {
    reader: R,
    partnership_grants: P,
}

pub struct GetSystemShopifySourceHandler<R> {
    reader: R,
}
impl<R, P> GetShopifySourceHandler<R, P> {
    pub fn new(reader: R, partnership_grants: P) -> Self {
        Self {
            reader,
            partnership_grants,
        }
    }
}

impl<R> GetSystemShopifySourceHandler<R> {
    pub fn new(reader: R) -> Self {
        Self { reader }
    }
}
#[async_trait::async_trait]
impl<R, P> GetShopifySourceUseCase for GetShopifySourceHandler<R, P>
where
    R: ShopifySourceReader,
    P: PartnershipGrantPolicy,
{
    #[tracing::instrument(name = "get_shopify_source", skip_all, fields(principal_type = context.principal.kind(), actor_id = tracing::field::Empty, request_id = %context.request_id, correlation_id = %context.correlation_id))]
    async fn execute(
        &self,
        context: &OperationContext,
        request: GetShopifySourceRequest,
    ) -> Result<GetShopifySourceResult, GetShopifySourceError> {
        tracing::Span::current().record(
            "actor_id",
            tracing::field::display(context.principal.label()),
        );
        let source = self
            .reader
            .find_by_domain(&request.domain)
            .await
            .map_err(map_read)?
            .ok_or(GetShopifySourceError::NotFound)?;
        if !self
            .partnership_grants
            .can_access_source(&context.principal, source.listing_source_id)
            .await
            .map_err(map_read)?
        {
            return Err(GetShopifySourceError::PartnershipGrantRequired);
        }
        Ok(source)
    }
}
#[async_trait::async_trait]
impl<R> GetShopifySourceUseCase for GetSystemShopifySourceHandler<R>
where
    R: ShopifySourceReader,
{
    #[tracing::instrument(name = "get_system_shopify_source", skip_all, fields(request_id = %context.request_id, correlation_id = %context.correlation_id))]
    async fn execute(
        &self,
        context: &OperationContext,
        request: GetShopifySourceRequest,
    ) -> Result<GetShopifySourceResult, GetShopifySourceError> {
        self.reader
            .find_by_domain(&request.domain)
            .await
            .map_err(map_read)?
            .ok_or(GetShopifySourceError::NotFound)
    }
}

fn map_read(error: ListingSourceReadError) -> GetShopifySourceError {
    match error {
        ListingSourceReadError::TemporarilyUnavailable { source } => {
            GetShopifySourceError::TemporarilyUnavailable { source }
        }
        ListingSourceReadError::InvalidReadModel { source } => {
            GetShopifySourceError::InvalidReadModel { source }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use application::operation_context::{CorrelationId, Principal, RequestId};
    struct Reader(ShopifySource);
    #[async_trait::async_trait]
    impl ShopifySourceReader for Reader {
        async fn find_by_domain(
            &self,
            _: &Domain,
        ) -> Result<Option<ShopifySource>, ListingSourceReadError> {
            Ok(Some(self.0.clone()))
        }
    }
    struct Grants(bool);
    #[async_trait::async_trait]
    impl PartnershipGrantPolicy for Grants {
        async fn can_access_source(
            &self,
            _: &Principal,
            _: listing_source_core::ListingSourceId,
        ) -> Result<bool, ListingSourceReadError> {
            Ok(self.0)
        }
    }
    fn source() -> ShopifySource {
        ShopifySource {
            listing_source_id: listing_source_core::ListingSourceId::new(),
            domain: Domain::try_from("shop.example")
                .unwrap_or_else(|error| panic!("valid test domain: {error}")),
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
    async fn should_return_shopify_source_for_system_intake() {
        let source = source();
        let expected_id = source.listing_source_id;
        let handler = GetSystemShopifySourceHandler::new(Reader(source.clone()));

        let result = handler
            .execute(
                &context(),
                GetShopifySourceRequest {
                    domain: source.domain,
                },
            )
            .await;

        assert!(matches!(result, Ok(result) if result.listing_source_id == expected_id));
    }

    #[tokio::test]
    async fn should_reject_shopify_source_without_partnership_grant() {
        let source = source();
        let handler = GetShopifySourceHandler::new(Reader(source.clone()), Grants(false));
        let result = handler
            .execute(
                &context(),
                GetShopifySourceRequest {
                    domain: source.domain,
                },
            )
            .await;
        assert!(matches!(
            result,
            Err(GetShopifySourceError::PartnershipGrantRequired)
        ));
    }
    #[tokio::test]
    async fn should_return_shopify_source_with_partnership_grant() {
        let source = source();
        let expected_id = source.listing_source_id;
        let handler = GetShopifySourceHandler::new(Reader(source.clone()), Grants(true));
        let result = handler
            .execute(
                &context(),
                GetShopifySourceRequest {
                    domain: source.domain,
                },
            )
            .await;
        assert!(matches!(result, Ok(result) if result.listing_source_id == expected_id));
    }
}
