use common::oauth_client_id::OAuthClientId;
use oauth::core::authorization_code::{AuthorizationCode, CodeChallengeMethod, OAuthCodeChallenge};
use oauth::core::client::{OAuthClient, OAuthClientName};
use oauth::dynamodb::authorization_code_record::AuthorizationCodeRecord;
use oauth::dynamodb::client_record::OAuthClientRecord;
use oauth::dynamodb::client_record_update::OAuthClientRecordUpdate;
use oauth::dynamodb::repository::{OAuthDynamoDbRepositoryImpl, OAuthRepository};
use std::collections::HashSet;
use test_api::*;
use time::{Duration, OffsetDateTime};
use user::core::access_token::{RawOAuthClientSecret, Scope};
use user::dynamodb::access_token_record::ScopeRecord;

fn now() -> OffsetDateTime {
    OffsetDateTime::from_unix_timestamp(OffsetDateTime::now_utc().unix_timestamp()).unwrap()
}

fn oauth_client() -> OAuthClient {
    let now = now();
    OAuthClient {
        client_id: OAuthClientId::new(),
        hashed_client_secret: RawOAuthClientSecret::new().into(),
        name: OAuthClientName::from("Test client"),
        redirect_uris: HashSet::from([url::Url::parse("https://client.example/callback").unwrap()]),
        scopes: HashSet::from([Scope::ProductsWrite]),
        created_by: common::user_id::UserId::new(),
        created: now,
        updated: now,
    }
}

fn authorization_code(client: &OAuthClient) -> AuthorizationCode {
    let now = now();
    AuthorizationCode {
        code: oauth::core::authorization_code::OAuthAuthorizationCode::new(),
        client_id: client.client_id,
        user_id: client.created_by,
        redirect_uri: url::Url::parse("https://client.example/callback").unwrap(),
        scopes: HashSet::from([Scope::ProductsWrite]),
        code_challenge: OAuthCodeChallenge::from("challenge"),
        code_challenge_method: CodeChallengeMethod::S256,
        expires: now + Duration::minutes(10),
        created: now,
    }
}

async fn repository() -> OAuthDynamoDbRepositoryImpl<'static> {
    OAuthDynamoDbRepositoryImpl::new(get_dynamodb_client().await, "table_1")
}

#[localstack_test(services = [DynamoDB()])]
async fn should_put_and_get_client_record() {
    let repository = repository().await;
    let client = oauth_client();

    repository
        .put_client_record(OAuthClientRecord::from(client.clone()))
        .await
        .unwrap();

    let actual = repository
        .get_client_record(&client.client_id)
        .await
        .unwrap()
        .map(OAuthClient::from)
        .unwrap();
    assert_eq!(client, actual);
}

#[localstack_test(services = [DynamoDB()])]
async fn should_query_client_records() {
    let repository = repository().await;
    let client = oauth_client();
    repository
        .put_client_record(OAuthClientRecord::from(client.clone()))
        .await
        .unwrap();

    let queried = repository
        .query_client_records()
        .await
        .unwrap()
        .into_iter()
        .map(OAuthClient::from)
        .collect::<Vec<_>>();

    assert_eq!(vec![client], queried);
}

#[localstack_test(services = [DynamoDB()])]
async fn should_update_client_record() {
    let repository = repository().await;
    let client = oauth_client();
    repository
        .put_client_record(OAuthClientRecord::from(client.clone()))
        .await
        .unwrap();

    let updated = repository
        .update_client_record(
            &client.client_id,
            OAuthClientRecordUpdate {
                name: Some(OAuthClientName::from("Updated client")),
                redirect_uris: Some(HashSet::from([url::Url::parse(
                    "https://client.example/updated-callback",
                )
                .unwrap()])),
                scopes: Some(HashSet::from([ScopeRecord::ShopsManage])),
                updated: now() + Duration::seconds(1),
            },
        )
        .await
        .unwrap()
        .map(OAuthClient::from)
        .unwrap();

    assert_eq!(OAuthClientName::from("Updated client"), updated.name);
    assert_eq!(
        HashSet::from([url::Url::parse("https://client.example/updated-callback").unwrap()]),
        updated.redirect_uris
    );
    assert_eq!(HashSet::from([Scope::ShopsManage]), updated.scopes);
}

#[localstack_test(services = [DynamoDB()])]
async fn should_delete_client_record() {
    let repository = repository().await;
    let client = oauth_client();
    repository
        .put_client_record(OAuthClientRecord::from(client.clone()))
        .await
        .unwrap();

    repository
        .delete_client_record(&client.client_id)
        .await
        .unwrap();

    assert!(
        repository
            .get_client_record(&client.client_id)
            .await
            .unwrap()
            .is_none()
    );
}

#[localstack_test(services = [DynamoDB()])]
async fn should_put_and_get_authorization_code_record() {
    let repository = repository().await;
    let client = oauth_client();
    let code = authorization_code(&client);

    repository
        .put_authorization_code_record(AuthorizationCodeRecord::from(code.clone()))
        .await
        .unwrap();

    let actual = repository
        .get_authorization_code_record(&code.code)
        .await
        .unwrap()
        .map(AuthorizationCode::from)
        .unwrap();
    assert_eq!(code, actual);
}

#[localstack_test(services = [DynamoDB()])]
async fn should_delete_authorization_code_record() {
    let repository = repository().await;
    let client = oauth_client();
    let code = authorization_code(&client);
    repository
        .put_authorization_code_record(AuthorizationCodeRecord::from(code.clone()))
        .await
        .unwrap();

    repository
        .delete_authorization_code_record(&code.code)
        .await
        .unwrap();

    assert!(
        repository
            .get_authorization_code_record(&code.code)
            .await
            .unwrap()
            .is_none()
    );
}
