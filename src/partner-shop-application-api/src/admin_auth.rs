use common::api::error::ApiError;
use common::api::error_code::FORBIDDEN;
use common::user_id::UserId;
use user::core::role::UserRole;
use user::service::user_service::UserService;

pub async fn require_admin(
    user_id: &UserId,
    user_service: &(impl UserService + Sync),
) -> Result<(), ApiError> {
    let user = user_service.find_user(user_id).await?;
    if user.role != UserRole::Admin {
        return Err(ApiError::forbidden(FORBIDDEN));
    }
    Ok(())
}
