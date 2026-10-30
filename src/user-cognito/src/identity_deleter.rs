use application::error::{box_error, static_error};
use aws_sdk_cognitoidentityprovider::{
    Client,
    error::{ProvideErrorMetadata, SdkError},
    types::UserType,
};
use user_service::ports::user_identity_deletion::{
    IdentityProviderFailure, UserIdentityDeleter, UserIdentityDeletionError, UserIdentityUsername,
};
use user_service::ports::{CognitoIdentity, CognitoSubject};

pub struct CognitoUserIdentityDeleter {
    client: Client,
    user_pool_id: String,
    issuer: Option<String>,
}

impl CognitoUserIdentityDeleter {
    pub fn new(client: Client, user_pool_id: String) -> Self {
        let issuer = client.config().region().map(|region| {
            format!(
                "https://cognito-idp.{}.amazonaws.com/{user_pool_id}",
                region.as_ref()
            )
        });
        Self {
            client,
            user_pool_id,
            issuer,
        }
    }
}

#[async_trait::async_trait]
impl UserIdentityDeleter for CognitoUserIdentityDeleter {
    async fn resolve_username(
        &self,
        identity: &CognitoIdentity,
    ) -> Result<Option<UserIdentityUsername>, UserIdentityDeletionError> {
        self.validate_issuer(identity)?;
        let mut pagination_token = None;
        let mut username = None;
        loop {
            let response = self
                .client
                .list_users()
                .user_pool_id(&self.user_pool_id)
                .filter(crate::subject_filter(&identity.subject))
                .attributes_to_get("sub")
                .limit(2)
                .set_pagination_token(pagination_token)
                .send()
                .await
                .map_err(provider_error)?;
            if let Some(found) = deletion_username(response.users(), &identity.subject)? {
                if username.is_some() {
                    return Err(invalid("ambiguous identity deletion lookup"));
                }
                username = Some(
                    UserIdentityUsername::try_from(found.to_owned())
                        .map_err(|source| invalid_response(box_error(source)))?,
                );
            }
            pagination_token = response.pagination_token().map(str::to_owned);
            if pagination_token.is_none() {
                return Ok(username);
            }
        }
    }
    async fn delete_identity(
        &self,
        identity: &CognitoIdentity,
        username: &UserIdentityUsername,
    ) -> Result<(), UserIdentityDeletionError> {
        self.validate_issuer(identity)?;
        let current = match self
            .client
            .admin_get_user()
            .user_pool_id(&self.user_pool_id)
            .username(username.as_str())
            .send()
            .await
        {
            Ok(current) => current,
            Err(source)
                if source
                    .as_service_error()
                    .is_some_and(|error| error.is_user_not_found_exception()) =>
            {
                return Ok(());
            }
            Err(source) => return Err(provider_error(source)),
        };
        if !exact_subject(current.user_attributes(), &identity.subject) {
            return Err(invalid("identity deletion subject changed"));
        }
        match self
            .client
            .admin_delete_user()
            .user_pool_id(&self.user_pool_id)
            .username(username.as_str())
            .send()
            .await
        {
            Ok(_) => Ok(()),
            Err(source)
                if source
                    .as_service_error()
                    .is_some_and(|error| error.is_user_not_found_exception()) =>
            {
                Ok(())
            }
            Err(source) => Err(provider_error(source)),
        }
    }
}
impl CognitoUserIdentityDeleter {
    fn validate_issuer(&self, identity: &CognitoIdentity) -> Result<(), UserIdentityDeletionError> {
        if self.issuer.as_deref() != Some(identity.issuer.as_str()) {
            return Err(UserIdentityDeletionError::IdentityProvider {
                failure: IdentityProviderFailure::Configuration,
                http_status: None,
                source: static_error("identity deletion issuer does not match configured pool"),
            });
        }
        Ok(())
    }
}

fn deletion_username<'a>(
    users: &'a [UserType],
    subject: &CognitoSubject,
) -> Result<Option<&'a str>, UserIdentityDeletionError> {
    let [user] = users else {
        return if users.is_empty() {
            Ok(None)
        } else {
            Err(invalid("ambiguous identity deletion lookup"))
        };
    };
    if !exact_subject(user.attributes(), subject) {
        return Err(invalid("identity deletion lookup subject mismatch"));
    }
    user.username()
        .filter(|value| !value.is_empty())
        .map(Some)
        .ok_or_else(|| invalid("identity deletion lookup has no username"))
}

fn exact_subject(
    attributes: &[aws_sdk_cognitoidentityprovider::types::AttributeType],
    subject: &CognitoSubject,
) -> bool {
    let mut subjects = attributes
        .iter()
        .filter(|attribute| attribute.name() == "sub");
    subjects
        .next()
        .is_some_and(|attribute| attribute.value() == Some(subject.as_str()))
        && subjects.next().is_none()
}

fn invalid(message: &'static str) -> UserIdentityDeletionError {
    invalid_response(static_error(message))
}

fn invalid_response(source: application::error::BoxError) -> UserIdentityDeletionError {
    UserIdentityDeletionError::IdentityProvider {
        failure: IdentityProviderFailure::InvalidResponse,
        http_status: None,
        source,
    }
}

/// Classifies an SDK failure by its transport outcome, HTTP status and standardized error code.
/// The message, raw response and any other code stay in the source chain.
fn provider_error<E>(source: SdkError<E>) -> UserIdentityDeletionError
where
    E: ProvideErrorMetadata + std::error::Error + Send + Sync + 'static,
{
    let http_status = source
        .raw_response()
        .map(|response| response.status().as_u16());
    let failure = match &source {
        SdkError::TimeoutError(_) => IdentityProviderFailure::Timeout,
        SdkError::DispatchFailure(error) if error.is_timeout() => IdentityProviderFailure::Timeout,
        SdkError::DispatchFailure(error) if error.is_io() => IdentityProviderFailure::Unavailable,
        SdkError::ConstructionFailure(_) | SdkError::DispatchFailure(_) => {
            IdentityProviderFailure::Configuration
        }
        SdkError::ServiceError(error) => match (http_status, error.err().code()) {
            (Some(429), _) | (_, Some("TooManyRequestsException" | "ThrottlingException")) => {
                IdentityProviderFailure::Throttled
            }
            (Some(401 | 403), _)
            | (
                _,
                Some(
                    "NotAuthorizedException"
                    | "AccessDeniedException"
                    | "UnrecognizedClientException"
                    | "ExpiredTokenException",
                ),
            ) => IdentityProviderFailure::Authentication,
            (Some(500..=599), _) | (_, Some("InternalErrorException")) => {
                IdentityProviderFailure::Unavailable
            }
            _ => IdentityProviderFailure::Configuration,
        },
        SdkError::ResponseError(_) => match http_status {
            Some(500..=599) => IdentityProviderFailure::Unavailable,
            _ => IdentityProviderFailure::InvalidResponse,
        },
        _ => IdentityProviderFailure::Unavailable,
    };
    UserIdentityDeletionError::IdentityProvider {
        failure,
        http_status,
        source: box_error(source),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aws_sdk_cognitoidentityprovider::config::{
        Credentials, Region, retry::RetryConfig, timeout::TimeoutConfig,
    };
    use serde_json::{Value, json};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use user_service::ports::CognitoIssuer;

    fn identity() -> CognitoIdentity {
        CognitoIdentity {
            issuer: CognitoIssuer::try_from(
                "https://cognito-idp.eu-central-1.amazonaws.com/eu-central-1_test",
            )
            .unwrap(),
            subject: CognitoSubject::try_from("old-subject").unwrap(),
        }
    }

    fn listed_user() -> Value {
        json!({"Username": "Google_provider-user", "Attributes": [{"Name": "sub", "Value": "old-subject"}]})
    }

    fn current_user(subject: &str) -> Value {
        json!({"Username": "Google_provider-user", "UserAttributes": [{"Name": "sub", "Value": subject}]})
    }

    async fn server(
        responses: Vec<(&'static str, u16, Value)>,
    ) -> (
        CognitoUserIdentityDeleter,
        tokio::task::JoinHandle<Vec<Value>>,
    ) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        let task = tokio::spawn(async move {
            let mut requests = Vec::new();
            for (operation, status, body) in responses {
                let (mut stream, _) =
                    tokio::time::timeout(std::time::Duration::from_secs(5), listener.accept())
                        .await
                        .unwrap()
                        .unwrap();
                let mut bytes = Vec::new();
                let mut buffer = [0; 4096];
                loop {
                    let n = stream.read(&mut buffer).await.unwrap();
                    assert_ne!(n, 0);
                    bytes.extend_from_slice(&buffer[..n]);
                    if let Some(end) = bytes.windows(4).position(|window| window == b"\r\n\r\n") {
                        let headers = std::str::from_utf8(&bytes[..end])
                            .unwrap()
                            .to_ascii_lowercase();
                        let length: usize = headers
                            .lines()
                            .find_map(|line| line.strip_prefix("content-length:"))
                            .unwrap()
                            .trim()
                            .parse()
                            .unwrap();
                        if bytes.len() >= end + 4 + length {
                            assert!(headers.contains(&format!(
                                "x-amz-target: awscognitoidentityproviderservice.{}",
                                operation.to_ascii_lowercase()
                            )));
                            requests.push(
                                serde_json::from_slice(&bytes[end + 4..end + 4 + length]).unwrap(),
                            );
                            break;
                        }
                    }
                }
                let body = body.to_string();
                let response = format!(
                    "HTTP/1.1 {status} response\r\nContent-Type: application/x-amz-json-1.1\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                stream.write_all(response.as_bytes()).await.unwrap();
            }
            requests
        });
        let client = Client::from_conf(
            aws_sdk_cognitoidentityprovider::Config::builder()
                .behavior_version_latest()
                .region(Region::new("eu-central-1"))
                .credentials_provider(Credentials::new(
                    "test",
                    "test",
                    None,
                    None,
                    "synthetic-test",
                ))
                .endpoint_url(endpoint)
                .retry_config(RetryConfig::disabled())
                .timeout_config(
                    TimeoutConfig::builder()
                        .operation_timeout(std::time::Duration::from_secs(2))
                        .build(),
                )
                .build(),
        );
        (
            CognitoUserIdentityDeleter::new(client, "eu-central-1_test".into()),
            task,
        )
    }

    async fn run_deletion(
        deleter: &CognitoUserIdentityDeleter,
        identity: &CognitoIdentity,
    ) -> Result<(), UserIdentityDeletionError> {
        let username = deleter
            .resolve_username(identity)
            .await?
            .ok_or_else(|| invalid("unobserved identity"))?;
        deleter.delete_identity(identity, &username).await
    }

    #[tokio::test]
    async fn deletes_verified_subject_using_provider_username_and_accepts_duplicate_deletion() {
        for (status, response) in [
            (200, json!({})),
            (
                400,
                json!({"__type": "UserNotFoundException", "message": "already deleted"}),
            ),
        ] {
            let (deleter, task) = server(vec![
                ("ListUsers", 200, json!({"Users": [listed_user()]})),
                ("AdminGetUser", 200, current_user("old-subject")),
                ("AdminDeleteUser", status, response),
            ])
            .await;
            run_deletion(&deleter, &identity()).await.unwrap();
            let requests = task.await.unwrap();
            assert_eq!(
                json!({"UserPoolId": "eu-central-1_test", "Filter": "sub = \"old-subject\"", "AttributesToGet": ["sub"], "Limit": 2}),
                requests[0]
            );
            for request in &requests[1..] {
                assert_eq!(
                    json!({"UserPoolId": "eu-central-1_test", "Username": "Google_provider-user"}),
                    *request
                );
            }
        }
    }

    #[tokio::test]
    async fn rejects_ambiguous_or_mismatched_lookups_before_deletion() {
        for body in [
            json!({"Users": [listed_user(), listed_user()]}),
            json!({"Users": [{"Username": "replacement", "Attributes": [{"Name": "sub", "Value": "new-subject"}]}]}),
            json!({"Users": [{"Username": "missing-subject"}]}),
            json!({"Users": [{"Attributes": [{"Name": "sub", "Value": "old-subject"}]}]}),
        ] {
            let (deleter, task) = server(vec![("ListUsers", 200, body)]).await;
            assert!(run_deletion(&deleter, &identity()).await.is_err());
            assert_eq!(1, task.await.unwrap().len());
        }
        let (deleter, task) = server(vec![
            ("ListUsers", 200, json!({"Users": [listed_user()]})),
            ("AdminGetUser", 200, current_user("new-subject")),
        ])
        .await;
        assert!(run_deletion(&deleter, &identity()).await.is_err());
        assert_eq!(2, task.await.unwrap().len());
    }

    #[tokio::test]
    async fn resolves_all_pages_including_empty_pages_before_deleting() {
        let (deleter, task) = server(vec![
            (
                "ListUsers",
                200,
                json!({"Users": [], "PaginationToken": "first"}),
            ),
            (
                "ListUsers",
                200,
                json!({"Users": [listed_user()], "PaginationToken": "last"}),
            ),
            ("ListUsers", 200, json!({"Users": []})),
            ("AdminGetUser", 200, current_user("old-subject")),
            ("AdminDeleteUser", 200, json!({})),
        ])
        .await;
        run_deletion(&deleter, &identity()).await.unwrap();
        let requests = task.await.unwrap();
        assert!(requests[0].get("PaginationToken").is_none());
        assert_eq!("first", requests[1]["PaginationToken"]);
        assert_eq!("last", requests[2]["PaginationToken"]);
        assert_eq!(5, requests.len());
    }

    #[tokio::test]
    async fn ambiguity_or_failure_on_later_pages_never_deletes() {
        for (status, body) in [
            (200, json!({"Users": [listed_user()]})),
            (400, json!({"__type": "TooManyRequestsException"})),
        ] {
            let (deleter, task) = server(vec![
                (
                    "ListUsers",
                    200,
                    json!({"Users": [listed_user()], "PaginationToken": "more"}),
                ),
                ("ListUsers", status, body),
            ])
            .await;
            assert!(run_deletion(&deleter, &identity()).await.is_err());
            assert_eq!(2, task.await.unwrap().len());
        }
    }

    #[tokio::test]
    async fn empty_lookup_is_not_completion_and_provider_failures_remain_retryable() {
        let (deleter, task) = server(vec![("ListUsers", 200, json!({"Users": []}))]).await;
        assert!(
            deleter
                .resolve_username(&identity())
                .await
                .unwrap()
                .is_none()
        );
        task.await.unwrap();
        let (deleter, task) = server(vec![
            ("ListUsers", 200, json!({"Users": [listed_user()]})),
            (
                "AdminGetUser",
                400,
                json!({"__type": "UserNotFoundException"}),
            ),
        ])
        .await;
        run_deletion(&deleter, &identity()).await.unwrap();
        task.await.unwrap();
        for (code, expected) in [
            (
                "TooManyRequestsException",
                IdentityProviderFailure::Throttled,
            ),
            (
                "NotAuthorizedException",
                IdentityProviderFailure::Authentication,
            ),
            (
                "ResourceNotFoundException",
                IdentityProviderFailure::Configuration,
            ),
        ] {
            let (deleter, task) = server(vec![(
                "ListUsers",
                400,
                json!({"__type": code, "message": "SENTINEL_PROVIDER_MESSAGE"}),
            )])
            .await;
            let error = run_deletion(&deleter, &identity()).await.unwrap_err();
            let UserIdentityDeletionError::IdentityProvider {
                failure,
                http_status,
                ..
            } = &error
            else {
                panic!("expected an identity provider failure: {error:?}");
            };
            assert_eq!((expected, Some(400)), (*failure, *http_status));
            assert!(std::error::Error::source(&error).is_some());
            task.await.unwrap();
        }
        let (deleter, task) = server(vec![("ListUsers", 503, json!({}))]).await;
        let error = run_deletion(&deleter, &identity()).await.unwrap_err();
        assert!(matches!(
            error,
            UserIdentityDeletionError::IdentityProvider {
                failure: IdentityProviderFailure::Unavailable,
                http_status: Some(503),
                ..
            }
        ));
        task.await.unwrap();
    }

    #[tokio::test]
    async fn classifies_an_unexpected_identity_as_an_invalid_provider_response() {
        let (deleter, task) = server(vec![(
            "ListUsers",
            200,
            json!({"Users": [listed_user(), listed_user()]}),
        )])
        .await;

        let error = run_deletion(&deleter, &identity()).await.unwrap_err();

        assert!(matches!(
            error,
            UserIdentityDeletionError::IdentityProvider {
                failure: IdentityProviderFailure::InvalidResponse,
                http_status: None,
                ..
            }
        ));
        task.await.unwrap();
    }

    #[tokio::test]
    async fn lost_delete_response_reconciles_saved_username_without_eventually_consistent_lookup() {
        let (deleter, task) = server(vec![(
            "AdminGetUser",
            400,
            json!({"__type": "UserNotFoundException"}),
        )])
        .await;
        let username = UserIdentityUsername::try_from("Google_provider-user".to_owned()).unwrap();
        deleter
            .delete_identity(&identity(), &username)
            .await
            .unwrap();
        assert_eq!(1, task.await.unwrap().len());
    }

    #[tokio::test]
    async fn rejects_another_issuer_without_contacting_provider() {
        let (deleter, task) = server(Vec::new()).await;
        let mut wrong = identity();
        wrong.issuer = CognitoIssuer::try_from("https://another-pool").unwrap();
        let error = run_deletion(&deleter, &wrong).await.unwrap_err();
        assert!(matches!(
            error,
            UserIdentityDeletionError::IdentityProvider {
                failure: IdentityProviderFailure::Configuration,
                ..
            }
        ));
        assert_eq!(
            Some("identity deletion issuer does not match configured pool"),
            std::error::Error::source(&error)
                .map(ToString::to_string)
                .as_deref()
        );
        assert!(task.await.unwrap().is_empty());
    }
}
