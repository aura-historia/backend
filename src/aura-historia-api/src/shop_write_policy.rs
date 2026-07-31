use common::error::boxed::static_error;
use common::operation_context::{OperationContext, Principal};
use common::shop_id::ShopId;
use common::user_id::UserId;
use shop_service::ports::{ShopWritePolicy, ShopWritePolicyError};
use shop_service::use_cases::queries::check_user_partner_shop::{
    CheckUserPartnerShopError, CheckUserPartnerShopRequest, CheckUserPartnerShopUseCase,
};
use std::sync::Arc;
use user_service::use_cases::queries::check_user_admin::{
    CheckUserAdminError, CheckUserAdminRequest, CheckUserAdminUseCase,
};

#[derive(Clone)]
pub struct ShopWritePolicyAdapter {
    check_user_admin: Arc<dyn CheckUserAdminUseCase>,
    check_user_partner_shop: Arc<dyn CheckUserPartnerShopUseCase>,
}

impl ShopWritePolicyAdapter {
    pub fn new(
        check_user_admin: impl CheckUserAdminUseCase + 'static,
        check_user_partner_shop: impl CheckUserPartnerShopUseCase + 'static,
    ) -> Self {
        Self {
            check_user_admin: Arc::new(check_user_admin),
            check_user_partner_shop: Arc::new(check_user_partner_shop),
        }
    }

    fn user_id(context: &OperationContext) -> Result<Option<UserId>, ShopWritePolicyError> {
        match context.principal {
            Principal::Anonymous => Err(ShopWritePolicyError::Forbidden),
            Principal::User(user_id) | Principal::DelegatedUser { user_id, .. } => {
                Ok(Some(user_id))
            }
            Principal::Service(_) | Principal::System => Ok(None),
        }
    }
}

#[async_trait::async_trait]
impl ShopWritePolicy for ShopWritePolicyAdapter {
    async fn ensure_can_create_shop(
        &self,
        context: &OperationContext,
    ) -> Result<(), ShopWritePolicyError> {
        let Some(user_id) = Self::user_id(context)? else {
            return Ok(());
        };
        self.check_user_admin
            .execute(context, CheckUserAdminRequest { user_id })
            .await
            .map(drop)
            .map_err(map_admin_error)
    }

    async fn ensure_can_update_shop(
        &self,
        context: &OperationContext,
        shop_id: ShopId,
    ) -> Result<(), ShopWritePolicyError> {
        let Some(user_id) = Self::user_id(context)? else {
            return Ok(());
        };
        match self
            .check_user_admin
            .execute(context, CheckUserAdminRequest { user_id })
            .await
        {
            Ok(_) => Ok(()),
            Err(CheckUserAdminError::Forbidden) => {
                let result = self
                    .check_user_partner_shop
                    .execute(context, CheckUserPartnerShopRequest { user_id, shop_id })
                    .await
                    .map_err(map_partner_error)?;
                if result.is_partner {
                    Ok(())
                } else {
                    Err(ShopWritePolicyError::Forbidden)
                }
            }
            Err(error) => Err(map_admin_error(error)),
        }
    }
}

fn map_admin_error(error: CheckUserAdminError) -> ShopWritePolicyError {
    match error {
        CheckUserAdminError::Forbidden | CheckUserAdminError::UserNotFound => {
            ShopWritePolicyError::Forbidden
        }
        CheckUserAdminError::TemporarilyUnavailable { source } => {
            ShopWritePolicyError::TemporarilyUnavailable { source }
        }
        CheckUserAdminError::InvalidReadModel { source }
        | CheckUserAdminError::Internal { source } => ShopWritePolicyError::Internal { source },
        CheckUserAdminError::BeginTransactionFailed
        | CheckUserAdminError::CommitTransactionFailed => {
            ShopWritePolicyError::TemporarilyUnavailable {
                source: static_error("check user admin transaction failed"),
            }
        }
    }
}

fn map_partner_error(error: CheckUserPartnerShopError) -> ShopWritePolicyError {
    match error {
        CheckUserPartnerShopError::Forbidden => ShopWritePolicyError::Forbidden,
        CheckUserPartnerShopError::TemporarilyUnavailable { source } => {
            ShopWritePolicyError::TemporarilyUnavailable { source }
        }
        CheckUserPartnerShopError::InvalidReadModel { source }
        | CheckUserPartnerShopError::Internal { source } => {
            ShopWritePolicyError::Internal { source }
        }
        CheckUserPartnerShopError::BeginTransactionFailed
        | CheckUserPartnerShopError::CommitTransactionFailed => {
            ShopWritePolicyError::TemporarilyUnavailable {
                source: static_error("check user partner shop transaction failed"),
            }
        }
    }
}
