use common::api::error::ApiError;
use common::user_id::UserId;
use user::service::user_service::UserService;

pub async fn check_admin(
    user_id: &UserId,
    user_service: &(impl UserService + Sync),
) -> Result<(), ApiError> {
    user_service.check_admin(user_id).await?;
    Ok(())
}
