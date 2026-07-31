use crate::auth::{ProtectedAuthExtractor, TransportPrincipal};
use crate::error::{ApiError, FORBIDDEN};
use crate::shops::shop_data::request_metadata;
use crate::state::ShopsState;
use axum::http::HeaderMap;
use axum::response::IntoResponse;
use common::operation_context::OperationContext;
use common::shop_id::ShopId;
use common::user_id::UserId;
use shop_service::use_cases::queries::check_user_partner_shop::CheckUserPartnerShopRequest;
use user_service::use_cases::queries::check_user_admin::CheckUserAdminRequest;

pub(crate) async fn protected_context(
    state: &ShopsState,
    headers: &HeaderMap,
) -> Result<(OperationContext, UserId), axum::response::Response> {
    let metadata = request_metadata(headers);
    let principal = ProtectedAuthExtractor::new(state.authenticator.as_ref())
        .extract(headers, &metadata)
        .await
        .map_err(|error| ApiError::from(error).into_response())?;
    let user_id = user_id(&principal).ok_or_else(|| {
        ApiError::forbidden(FORBIDDEN)
            .with_detail("User principal is required.")
            .into_response()
    })?;
    Ok((principal.operation_context(metadata), user_id))
}

pub(crate) async fn ensure_admin(
    state: &ShopsState,
    context: &OperationContext,
    user_id: UserId,
) -> Result<(), axum::response::Response> {
    state
        .check_user_admin
        .execute(context, CheckUserAdminRequest { user_id })
        .await
        .map(drop)
        .map_err(|error| ApiError::from(error).into_response())
}

pub(crate) async fn ensure_admin_or_partner(
    state: &ShopsState,
    context: &OperationContext,
    user_id: UserId,
    shop_id: ShopId,
) -> Result<(), axum::response::Response> {
    match state
        .check_user_admin
        .execute(context, CheckUserAdminRequest { user_id })
        .await
    {
        Ok(_) => Ok(()),
        Err(user_service::use_cases::queries::check_user_admin::CheckUserAdminError::Forbidden) => {
            let result = state
                .check_user_partner_shop
                .execute(context, CheckUserPartnerShopRequest { user_id, shop_id })
                .await
                .map_err(|error| ApiError::from(error).into_response())?;
            if result.is_partner {
                Ok(())
            } else {
                Err(ApiError::forbidden(FORBIDDEN)
                    .with_detail("User is not partner of this shop.")
                    .into_response())
            }
        }
        Err(error) => Err(ApiError::from(error).into_response()),
    }
}

fn user_id(principal: &TransportPrincipal) -> Option<UserId> {
    match principal {
        TransportPrincipal::Anonymous => None,
        TransportPrincipal::User { user_id, .. } => Some(*user_id),
    }
}
