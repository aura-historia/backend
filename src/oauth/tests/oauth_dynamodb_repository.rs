use oauth::core::authorization_code::{AuthorizationCode, CodeChallengeMethod, OAuthCodeChallenge};
use oauth::core::client::{OAuthClient, OAuthClientId, OAuthClientName, OAuthRedirectUri};
use oauth::dynamodb::authorization_code_record::AuthorizationCodeRecord;
use oauth::dynamodb::client_record::OAuthClientRecord;
use oauth::dynamodb::client_record_update::OAuthClientRecordUpdate;
use oauth::dynamodb::repository::{OAuthDynamoDbRepositoryImpl, OAuthRepository};
use test_api::*;
use time::{Duration, OffsetDateTime};
use user::core::access_token::{RawOAuthClientSecret, Scope};
use user::dynamodb::access_token_record::ScopeRecord;

#[localstack_test(services = [DynamoDB()])]
async fn should_crud_oauth_records() {
    let repository = OAuthDynamoDbRepositoryImpl::new(get_dynamodb_client().await, "table_1");
    let secret = RawOAuthClientSecret::new();
    let now =
        OffsetDateTime::from_unix_timestamp(OffsetDateTime::now_utc().unix_timestamp()).unwrap();
    let client = OAuthClient {
        client_id: OAuthClientId::from("client_1"),
        hashed_client_secret: secret.into(),
        name: OAuthClientName::from("Test client"),
        redirect_uris: HashSet::from([OAuthRedirectUri::from("https://client.example/callback")]),
        scopes: HashSet::from([Scope::ProductsWrite]),
        created_by: common::user_id::UserId::new(),
        created: now,
        updated: now,
    };

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
    let queried = repository
        .query_client_records(&client.created_by)
        .await
        .unwrap()
        .into_iter()
        .map(OAuthClient::from)
        .collect::<Vec<_>>();
    assert_eq!(vec![client.clone()], queried);

    let updated = repository
        .update_client_record(
            &client.client_id,
            OAuthClientRecordUpdate {
                name: Some(OAuthClientName::from("Updated client")),
                redirect_uris: Some(HashSet::from([OAuthRedirectUri::from(
                    "https://client.example/updated-callback",
                )])),
                scopes: Some(HashSet::from([ScopeRecord::ShopsManage])),
                updated: now + Duration::seconds(1),
            },
        )
        .await
        .unwrap()
        .map(OAuthClient::from)
        .unwrap();
    assert_eq!(OAuthClientName::from("Updated client"), updated.name);
    assert_eq!(
        HashSet::from([OAuthRedirectUri::from(
            "https://client.example/updated-callback"
        )]),
        updated.redirect_uris
    );
    assert_eq!(HashSet::from([Scope::ShopsManage]), updated.scopes);

    let code = AuthorizationCode {
        code: oauth::core::authorization_code::OAuthAuthorizationCode::new(),
        client_id: client.client_id.clone(),
        user_id: client.created_by,
        redirect_uri: OAuthRedirectUri::from("https://client.example/callback"),
        scopes: HashSet::from([Scope::ProductsWrite]),
        code_challenge: OAuthCodeChallenge::from("challenge"),
        code_challenge_method: CodeChallengeMethod::S256,
        expires: now + Duration::minutes(10),
        created: now,
    };
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
