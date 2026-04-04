use common::user_id::UserId;

#[derive(thiserror::Error, Debug)]
pub enum CognitoAdminError {
    #[error("Failed to delete Cognito user: {0}")]
    AdminDeleteUser(Box<dyn std::error::Error + Send + Sync>),
}

#[async_trait::async_trait]
#[mockall::automock]
pub trait CognitoAdminService {
    async fn admin_delete_user(&self, user_id: &UserId) -> Result<(), CognitoAdminError>;
}

#[cfg(feature = "cognito")]
pub mod cognito_impl {
    use super::{CognitoAdminError, CognitoAdminService};
    use aws_sdk_cognitoidentityprovider::Client;
    use common::user_id::UserId;

    pub struct CognitoAdminServiceImpl<'a> {
        client: &'a Client,
        user_pool_id: String,
    }

    impl<'a> CognitoAdminServiceImpl<'a> {
        pub fn new(client: &'a Client, user_pool_id: impl Into<String>) -> Self {
            Self {
                client,
                user_pool_id: user_pool_id.into(),
            }
        }
    }

    #[async_trait::async_trait]
    impl<'a> CognitoAdminService for CognitoAdminServiceImpl<'a> {
        async fn admin_delete_user(&self, user_id: &UserId) -> Result<(), CognitoAdminError> {
            self.client
                .admin_delete_user()
                .user_pool_id(&self.user_pool_id)
                .username(user_id.to_string())
                .send()
                .await
                .map_err(|e| CognitoAdminError::AdminDeleteUser(Box::new(e)))?;
            Ok(())
        }
    }
}
