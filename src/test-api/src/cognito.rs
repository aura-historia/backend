use crate::{IntegrationTestService, get_dynamodb_client, localstack::get_aws_config};
use aws_sdk_cognitoidentityprovider::types::{AttributeType, AuthFlowType, MessageActionType};
use aws_tests_common::get_cfn_output;
use common::actor::{RequestContext, domain::Actor};
use fake::{
    Fake,
    faker::internet::de_de::{Password, SafeEmail},
};
use tokio::sync::OnceCell;
use user::{
    dynamodb::repository::UserDynamoDbRepositoryImpl,
    service::{
        command::CreateUserCommand,
        user_service::{UserService, UserServiceImpl},
    },
};
use uuid::Uuid;

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

pub struct TestUser {
    pub access_token: String,
    pub id_token: String,
    pub sub: Uuid,
}
pub async fn create_random_test_user() -> TestUser {
    let email: String = SafeEmail().fake();

    create_test_user(&email).await
}

#[allow(clippy::too_many_arguments)]
pub async fn create_test_user(email: &str) -> TestUser {
    let cfn = get_cfn_output();
    let cognito = get_cognito_client().await;
    let password: String = format!("{}*1bC", Password(8..12).fake::<String>());

    let req_builder = cognito
        .admin_create_user()
        .user_pool_id(&cfn.cognito_user_pool_id)
        .username(email)
        .user_attributes(
            AttributeType::builder()
                .name("email")
                .value(email)
                .build()
                .unwrap(),
        )
        .message_action(MessageActionType::Suppress);

    let created = req_builder.send().await.unwrap();
    cognito
        .admin_set_user_password()
        .user_pool_id(&cfn.cognito_user_pool_id)
        .username(email)
        .password(&password)
        .permanent(true)
        .send()
        .await
        .unwrap();
    let auth = cognito
        .initiate_auth()
        .client_id(&cfn.cognito_user_pool_client_public_id)
        .auth_flow(AuthFlowType::UserPasswordAuth)
        .auth_parameters("USERNAME", email)
        .auth_parameters("PASSWORD", &password)
        .send()
        .await
        .unwrap()
        .authentication_result
        .unwrap();

    let sub: Uuid = created
        .user
        .unwrap()
        .attributes
        .unwrap()
        .into_iter()
        .find(|attr| attr.name == "sub")
        .unwrap()
        .value
        .unwrap()
        .try_into()
        .unwrap();

    let user_repository =
        UserDynamoDbRepositoryImpl::new(get_dynamodb_client().await, &cfn.dynamodb_table_1_name);
    let user_service = UserServiceImpl::new(&user_repository);
    let create_user_command = CreateUserCommand {
        id: sub.into(),
        email: email.try_into().unwrap(),
    };
    let _ = user_service
        .create_user(
            &RequestContext {
                actor: Actor::User(sub.into()),
            },
            create_user_command,
        )
        .await
        .unwrap();

    TestUser {
        access_token: auth.access_token.unwrap(),
        id_token: auth.id_token.unwrap(),
        sub,
    }
}
