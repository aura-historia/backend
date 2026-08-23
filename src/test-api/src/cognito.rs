use crate::{IntegrationTestService, localstack::get_aws_config};
use aws_tests_common::get_cfn_output;
use tokio::sync::OnceCell;

pub struct Cognito();

#[async_trait::async_trait]
impl IntegrationTestService for Cognito {
    fn service_names(&self) -> &'static [&'static str] {
        &["cognito-idp"]
    }

    async fn set_up(&self) {}

    async fn tear_down(&self) {
        let client = get_cognito_client().await;
        let mut pagination_token = None;

        loop {
            let resp = client
                .list_users()
                .user_pool_id(&get_cfn_output().cognito_user_pool_id)
                .set_pagination_token(pagination_token.clone())
                .limit(60) // max page size
                .send()
                .await
                .expect("shouldn't fail listing cognito users");

            for u in resp.users() {
                if let Some(username) = u.username() {
                    client
                        .admin_delete_user()
                        .user_pool_id(&get_cfn_output().cognito_user_pool_id)
                        .username(username)
                        .send()
                        .await
                        .expect("shouldn't fail deleting user");
                }
            }

            pagination_token = resp.pagination_token().map(|s| s.to_string());
            if pagination_token.is_none() {
                break;
            }
        }
    }
}

static COGNITO_CLIENT: OnceCell<aws_sdk_cognitoidentityprovider::Client> = OnceCell::const_new();
pub async fn get_cognito_client() -> &'static aws_sdk_cognitoidentityprovider::Client {
    COGNITO_CLIENT
        .get_or_init(|| async {
            aws_sdk_cognitoidentityprovider::Client::new(get_aws_config().await)
        })
        .await
}
