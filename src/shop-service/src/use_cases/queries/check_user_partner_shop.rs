use crate::ports::{PartnerShopReadError, PartnerShopReader};
use common::operation_context::{OperationContext, Principal};
use common::{shop_id::ShopId, user_id::UserId};

#[derive(Debug, Clone, PartialEq)]
pub struct CheckUserPartnerShopRequest {
    pub user_id: UserId,
    pub shop_id: ShopId,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CheckUserPartnerShopResult {
    pub user_id: UserId,
    pub shop_id: ShopId,
    pub is_partner: bool,
}

#[derive(Debug, thiserror::Error)]
pub enum CheckUserPartnerShopError {
    #[error("operation not permitted")]
    Forbidden,
    #[error("temporary partner shop read failure")]
    TemporarilyUnavailable,
    #[error("invalid partner shop read model")]
    InvalidReadModel,
    #[error("internal partner shop read failure")]
    Internal,
}

#[async_trait::async_trait]
pub trait CheckUserPartnerShopUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        request: CheckUserPartnerShopRequest,
    ) -> Result<CheckUserPartnerShopResult, CheckUserPartnerShopError>;
}

pub struct CheckUserPartnerShopHandler<R> {
    reader: R,
}

impl<R> CheckUserPartnerShopHandler<R> {
    pub fn new(reader: R) -> Self {
        Self { reader }
    }
}

#[async_trait::async_trait]
impl<R> CheckUserPartnerShopUseCase for CheckUserPartnerShopHandler<R>
where
    R: PartnerShopReader,
{
    #[tracing::instrument(
        name = "check_user_partner_shop",
        skip_all,
        fields(
            user_id = %request.user_id,
            shop_id = %request.shop_id,
            principal_type = context.principal.kind(),
            request_id = %context.request_id,
            correlation_id = %context.correlation_id,
        )
    )]
    async fn execute(
        &self,
        context: &OperationContext,
        request: CheckUserPartnerShopRequest,
    ) -> Result<CheckUserPartnerShopResult, CheckUserPartnerShopError> {
        authorize_check(context, request.user_id)?;
        let is_partner = self.reader.is_user_partner_of_shop(&request).await?;

        Ok(CheckUserPartnerShopResult {
            user_id: request.user_id,
            shop_id: request.shop_id,
            is_partner,
        })
    }
}

impl From<PartnerShopReadError> for CheckUserPartnerShopError {
    fn from(error: PartnerShopReadError) -> Self {
        match error {
            PartnerShopReadError::TemporarilyUnavailable => Self::TemporarilyUnavailable,
            PartnerShopReadError::InvalidReadModel => Self::InvalidReadModel,
            PartnerShopReadError::Internal => Self::Internal,
        }
    }
}

fn authorize_check(
    context: &OperationContext,
    requested_user_id: UserId,
) -> Result<(), CheckUserPartnerShopError> {
    match &context.principal {
        Principal::User(user_id) if *user_id == requested_user_id => Ok(()),
        Principal::Service(_) | Principal::System => Ok(()),
        Principal::Anonymous | Principal::User(_) => Err(CheckUserPartnerShopError::Forbidden),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use common::operation_context::{CorrelationId, RequestId};

    #[test]
    fn should_allow_user_to_check_own_partner_shop() {
        let user_id = UserId::new();
        let context = OperationContext {
            principal: Principal::User(user_id),
            request_id: RequestId::from("request"),
            correlation_id: CorrelationId::from("correlation"),
        };

        let result = authorize_check(&context, user_id);

        assert!(matches!(result, Ok(())));
    }

    #[test]
    fn should_reject_user_checking_other_user() {
        let context = OperationContext {
            principal: Principal::User(UserId::new()),
            request_id: RequestId::from("request"),
            correlation_id: CorrelationId::from("correlation"),
        };

        let result = authorize_check(&context, UserId::new());

        assert!(matches!(result, Err(CheckUserPartnerShopError::Forbidden)));
    }
}
