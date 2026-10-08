use std::sync::Arc;

use async_trait::async_trait;
use aws_sdk_cognitoidentityprovider::{
    Client,
    error::ProvideErrorMetadata,
    types::{ProviderUserIdentifierType, UserStatusType, UserType},
};
use serde_json::Value;
use user_service::ports::CognitoSubject;

const MAX_SUBJECT_LOOKUP_RESULTS: i32 = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum CognitoAccountLinkError {
    #[error("Cognito account linking is temporarily unavailable")]
    TemporarilyUnavailable,
    #[error("Cognito account linking encountered invalid account state")]
    InvalidState,
}

pub struct CognitoAccountLinker {
    provider: Arc<dyn CognitoAccountLinkProvider>,
}

impl CognitoAccountLinker {
    pub fn new(client: Client) -> Self {
        Self {
            provider: Arc::new(AwsCognitoAccountLinkProvider { client }),
        }
    }

    pub async fn link_account(
        &self,
        user_pool_id: &str,
        source_provider_name: &str,
        source_provider_attribute_name: &str,
        source_subject: &str,
        destination_subject: &CognitoSubject,
        canonical_email: &str,
    ) -> Result<(), CognitoAccountLinkError> {
        let lookup = self
            .provider
            .find_users_by_subject(
                user_pool_id,
                &subject_filter(destination_subject),
                MAX_SUBJECT_LOOKUP_RESULTS,
            )
            .await?;
        if lookup.has_more {
            return Err(CognitoAccountLinkError::InvalidState);
        }

        let [user] = lookup.users.as_slice() else {
            return Err(CognitoAccountLinkError::InvalidState);
        };

        require_bound_profile(user, destination_subject, canonical_email)?;
        let destination = destination_for(user)?;
        let source = ProviderIdentity {
            provider_name: source_provider_name.to_owned(),
            attribute_name: source_provider_attribute_name.to_owned(),
            attribute_value: source_subject.to_owned(),
        };

        self.provider
            .link_provider(user_pool_id, &source, &destination)
            .await
    }
}

#[derive(Default)]
struct SubjectLookup {
    users: Vec<UserType>,
    has_more: bool,
}

#[async_trait]
trait CognitoAccountLinkProvider: Send + Sync {
    async fn find_users_by_subject(
        &self,
        user_pool_id: &str,
        filter: &str,
        limit: i32,
    ) -> Result<SubjectLookup, CognitoAccountLinkError>;

    async fn link_provider(
        &self,
        user_pool_id: &str,
        source: &ProviderIdentity,
        destination: &ProviderIdentity,
    ) -> Result<(), CognitoAccountLinkError>;
}

struct AwsCognitoAccountLinkProvider {
    client: Client,
}

#[async_trait]
impl CognitoAccountLinkProvider for AwsCognitoAccountLinkProvider {
    async fn find_users_by_subject(
        &self,
        user_pool_id: &str,
        filter: &str,
        limit: i32,
    ) -> Result<SubjectLookup, CognitoAccountLinkError> {
        let response = self
            .client
            .list_users()
            .user_pool_id(user_pool_id)
            .filter(filter)
            .limit(limit)
            .send()
            .await
            .map_err(|error| {
                classify_cognito_error_code(
                    error
                        .as_service_error()
                        .and_then(|service_error| service_error.code()),
                )
            })?;

        Ok(SubjectLookup {
            users: response
                .users()
                .iter()
                .take(limit as usize)
                .cloned()
                .collect(),
            has_more: response.pagination_token().is_some(),
        })
    }

    async fn link_provider(
        &self,
        user_pool_id: &str,
        source: &ProviderIdentity,
        destination: &ProviderIdentity,
    ) -> Result<(), CognitoAccountLinkError> {
        let source_user = sdk_provider_user(source);
        let destination_user = sdk_provider_user(destination);

        self.client
            .admin_link_provider_for_user()
            .user_pool_id(user_pool_id)
            .source_user(source_user)
            .destination_user(destination_user)
            .send()
            .await
            .map_err(|error| {
                classify_cognito_error_code(
                    error
                        .as_service_error()
                        .and_then(|service_error| service_error.code()),
                )
            })?;

        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ProviderIdentity {
    provider_name: String,
    attribute_name: String,
    attribute_value: String,
}

fn sdk_provider_user(identity: &ProviderIdentity) -> ProviderUserIdentifierType {
    ProviderUserIdentifierType::builder()
        .provider_name(&identity.provider_name)
        .provider_attribute_name(&identity.attribute_name)
        .provider_attribute_value(&identity.attribute_value)
        .build()
}

fn subject_filter(subject: &CognitoSubject) -> String {
    let mut escaped = String::with_capacity(subject.as_str().len());
    for character in subject.as_str().chars() {
        if matches!(character, '\\' | '"') {
            escaped.push('\\');
        }
        escaped.push(character);
    }
    format!("sub = \"{escaped}\"")
}

fn require_bound_profile(
    user: &UserType,
    bound_subject: &CognitoSubject,
    canonical_email: &str,
) -> Result<(), CognitoAccountLinkError> {
    let subject = attribute(user, "sub")?
        .and_then(|attribute| attribute.value())
        .ok_or(CognitoAccountLinkError::InvalidState)?;
    if subject != bound_subject.as_str() {
        return Err(CognitoAccountLinkError::InvalidState);
    }

    let email = attribute(user, "email")?
        .and_then(|attribute| attribute.value())
        .ok_or(CognitoAccountLinkError::InvalidState)?;
    if email != canonical_email {
        return Err(CognitoAccountLinkError::InvalidState);
    }

    let verified = attribute(user, "email_verified")?
        .and_then(|attribute| attribute.value())
        .is_some_and(|value| value == "true");
    if !verified {
        return Err(CognitoAccountLinkError::InvalidState);
    }

    Ok(())
}

fn destination_for(user: &UserType) -> Result<ProviderIdentity, CognitoAccountLinkError> {
    let status = user
        .user_status()
        .ok_or(CognitoAccountLinkError::InvalidState)?;

    match status {
        UserStatusType::ExternalProvider => {
            let identities = attribute(user, "identities")?
                .and_then(|attribute| attribute.value())
                .ok_or(CognitoAccountLinkError::InvalidState)?;
            primary_external_identity(identities)?.ok_or(CognitoAccountLinkError::InvalidState)
        }
        UserStatusType::Confirmed
        | UserStatusType::Unconfirmed
        | UserStatusType::ForceChangePassword
        | UserStatusType::ResetRequired => {
            require_no_facebook_identity(user)?;
            let username = user
                .username()
                .filter(|username| !username.is_empty())
                .ok_or(CognitoAccountLinkError::InvalidState)?;
            Ok(ProviderIdentity {
                provider_name: "Cognito".to_owned(),
                attribute_name: "Cognito_Subject".to_owned(),
                attribute_value: username.to_owned(),
            })
        }
        _ => Err(CognitoAccountLinkError::InvalidState),
    }
}

fn primary_external_identity(
    identities_json: &str,
) -> Result<Option<ProviderIdentity>, CognitoAccountLinkError> {
    let identities: Value =
        serde_json::from_str(identities_json).map_err(|_| CognitoAccountLinkError::InvalidState)?;
    let identities = identities
        .as_array()
        .ok_or(CognitoAccountLinkError::InvalidState)?;
    if identities.is_empty() {
        return Ok(None);
    }

    let mut primary_identity = None;
    for identity in identities {
        let identity = identity
            .as_object()
            .ok_or(CognitoAccountLinkError::InvalidState)?;
        let provider_name = identity
            .get("providerName")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .ok_or(CognitoAccountLinkError::InvalidState)?;
        // Facebook-origin accounts are never eligible Google-link destinations, even if their
        // email is verified later through another Cognito flow.
        if provider_name == "Facebook" {
            return Err(CognitoAccountLinkError::InvalidState);
        }
        let user_id = identity
            .get("userId")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .ok_or(CognitoAccountLinkError::InvalidState)?;
        let is_primary = identity
            .get("primary")
            .and_then(primary_flag)
            .ok_or(CognitoAccountLinkError::InvalidState)?;

        if is_primary {
            if primary_identity.is_some() {
                return Err(CognitoAccountLinkError::InvalidState);
            }
            primary_identity = Some(ProviderIdentity {
                provider_name: provider_name.to_owned(),
                attribute_name: "Cognito_Subject".to_owned(),
                attribute_value: user_id.to_owned(),
            });
        }
    }

    primary_identity
        .map(Some)
        .ok_or(CognitoAccountLinkError::InvalidState)
}

fn primary_flag(value: &Value) -> Option<bool> {
    match value {
        Value::Bool(value) => Some(*value),
        Value::String(value) if value == "true" => Some(true),
        Value::String(value) if value == "false" => Some(false),
        _ => None,
    }
}

fn attribute<'a>(
    user: &'a UserType,
    name: &str,
) -> Result<
    Option<&'a aws_sdk_cognitoidentityprovider::types::AttributeType>,
    CognitoAccountLinkError,
> {
    let mut matching = user
        .attributes()
        .iter()
        .filter(|attribute| attribute.name() == name);
    let Some(attribute) = matching.next() else {
        return Ok(None);
    };
    if matching.next().is_some() {
        return Err(CognitoAccountLinkError::InvalidState);
    }
    Ok(Some(attribute))
}

fn require_no_facebook_identity(user: &UserType) -> Result<(), CognitoAccountLinkError> {
    let Some(identities) = attribute(user, "identities")? else {
        return Ok(());
    };
    let identities = identities
        .value()
        .ok_or(CognitoAccountLinkError::InvalidState)?;
    let identities: Value =
        serde_json::from_str(identities).map_err(|_| CognitoAccountLinkError::InvalidState)?;
    let identities = identities
        .as_array()
        .ok_or(CognitoAccountLinkError::InvalidState)?;
    for identity in identities {
        let provider_name = identity
            .as_object()
            .and_then(|identity| identity.get("providerName"))
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .ok_or(CognitoAccountLinkError::InvalidState)?;
        if provider_name == "Facebook" {
            return Err(CognitoAccountLinkError::InvalidState);
        }
    }
    Ok(())
}

fn classify_cognito_error_code(code: Option<&str>) -> CognitoAccountLinkError {
    if code.is_none_or(|code| {
        matches!(
            code,
            "InternalErrorException"
                | "TooManyRequestsException"
                | "LimitExceededException"
                | "ServiceUnavailableException"
                | "ThrottlingException"
                | "ConcurrentModificationException"
        )
    }) {
        CognitoAccountLinkError::TemporarilyUnavailable
    } else {
        CognitoAccountLinkError::InvalidState
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        future::Future,
        sync::Mutex,
        task::{Context, Poll, Waker},
    };
    use user_service::ports::CognitoIssuer;

    #[derive(Debug, Clone, PartialEq, Eq)]
    struct SearchCall {
        user_pool_id: String,
        filter: String,
        limit: i32,
    }

    #[derive(Debug, Clone, PartialEq, Eq)]
    struct LinkCall {
        user_pool_id: String,
        source: ProviderIdentity,
        destination: ProviderIdentity,
    }

    #[derive(Default)]
    struct FakeProvider {
        users: Vec<UserType>,
        has_more: bool,
        search_failure: Option<CognitoAccountLinkError>,
        link_failure: Option<CognitoAccountLinkError>,
        searches: Mutex<Vec<SearchCall>>,
        links: Mutex<Vec<LinkCall>>,
    }

    #[async_trait]
    impl CognitoAccountLinkProvider for FakeProvider {
        async fn find_users_by_subject(
            &self,
            user_pool_id: &str,
            filter: &str,
            limit: i32,
        ) -> Result<SubjectLookup, CognitoAccountLinkError> {
            self.searches
                .lock()
                .expect("search mutex")
                .push(SearchCall {
                    user_pool_id: user_pool_id.to_owned(),
                    filter: filter.to_owned(),
                    limit,
                });
            if let Some(error) = self.search_failure {
                return Err(error);
            }
            Ok(SubjectLookup {
                users: self.users.clone(),
                has_more: self.has_more,
            })
        }

        async fn link_provider(
            &self,
            user_pool_id: &str,
            source: &ProviderIdentity,
            destination: &ProviderIdentity,
        ) -> Result<(), CognitoAccountLinkError> {
            self.links.lock().expect("link mutex").push(LinkCall {
                user_pool_id: user_pool_id.to_owned(),
                source: source.clone(),
                destination: destination.clone(),
            });
            self.link_failure.map_or(Ok(()), Err)
        }
    }

    fn linker(provider: Arc<FakeProvider>) -> CognitoAccountLinker {
        CognitoAccountLinker { provider }
    }

    fn subject(value: &str) -> CognitoSubject {
        CognitoSubject::try_from(value)
            .unwrap_or_else(|error| panic!("invalid test subject: {error}"))
    }

    fn attribute_type(
        name: &str,
        value: &str,
    ) -> aws_sdk_cognitoidentityprovider::types::AttributeType {
        aws_sdk_cognitoidentityprovider::types::AttributeType::builder()
            .name(name)
            .value(value)
            .build()
            .expect("valid Cognito attribute")
    }

    fn user(
        status: UserStatusType,
        username: &str,
        subject: &str,
        email: &str,
        email_verified: &str,
        identities: Option<&str>,
    ) -> UserType {
        let mut attributes = vec![
            attribute_type("sub", subject),
            attribute_type("email", email),
            attribute_type("email_verified", email_verified),
        ];
        if let Some(identities) = identities {
            attributes.push(attribute_type("identities", identities));
        }
        let mut builder = UserType::builder().username(username).user_status(status);
        for attribute in attributes {
            builder = builder.attributes(attribute);
        }
        builder.build()
    }

    fn block_on<F: Future>(future: F) -> F::Output {
        let mut context = Context::from_waker(Waker::noop());
        let mut future = Box::pin(future);
        loop {
            match future.as_mut().poll(&mut context) {
                Poll::Ready(output) => return output,
                Poll::Pending => std::thread::yield_now(),
            }
        }
    }

    fn link(
        linker: &CognitoAccountLinker,
        destination_subject: &CognitoSubject,
        email: &str,
    ) -> Result<(), CognitoAccountLinkError> {
        block_on(linker.link_account(
            "pool",
            "Google",
            "Cognito_Subject",
            "google-source-id",
            destination_subject,
            email,
        ))
    }

    #[test]
    fn should_classify_retryable_and_deterministic_cognito_errors() {
        for code in [
            "InternalErrorException",
            "TooManyRequestsException",
            "LimitExceededException",
            "ServiceUnavailableException",
            "ThrottlingException",
            "ConcurrentModificationException",
        ] {
            assert_eq!(
                CognitoAccountLinkError::TemporarilyUnavailable,
                classify_cognito_error_code(Some(code)),
                "retryable Cognito error {code}"
            );
        }
        assert_eq!(
            CognitoAccountLinkError::TemporarilyUnavailable,
            classify_cognito_error_code(None)
        );
        for code in ["InvalidParameterException", "NotAuthorizedException"] {
            assert_eq!(
                CognitoAccountLinkError::InvalidState,
                classify_cognito_error_code(Some(code)),
                "deterministic Cognito error {code}"
            );
        }
    }

    #[test]
    fn should_fail_closed_on_subject_lookup_errors_without_linking() {
        for failure in [
            CognitoAccountLinkError::TemporarilyUnavailable,
            CognitoAccountLinkError::InvalidState,
        ] {
            let provider = Arc::new(FakeProvider {
                search_failure: Some(failure),
                ..FakeProvider::default()
            });

            assert_eq!(
                Err(failure),
                link(
                    &linker(provider.clone()),
                    &subject("canonical-sub"),
                    "person@example.test"
                )
            );
            assert_eq!(1, provider.searches.lock().expect("search mutex").len());
            assert!(provider.links.lock().expect("link mutex").is_empty());
        }
    }

    #[test]
    fn should_fail_closed_when_admin_link_errors() {
        for failure in [
            CognitoAccountLinkError::TemporarilyUnavailable,
            CognitoAccountLinkError::InvalidState,
        ] {
            let provider = Arc::new(FakeProvider {
                users: vec![user(
                    UserStatusType::Confirmed,
                    "native-user",
                    "canonical-sub",
                    "person@example.test",
                    "true",
                    None,
                )],
                link_failure: Some(failure),
                ..FakeProvider::default()
            });

            assert_eq!(
                Err(failure),
                link(
                    &linker(provider.clone()),
                    &subject("canonical-sub"),
                    "person@example.test"
                )
            );
            assert_eq!(1, provider.searches.lock().expect("search mutex").len());
            assert_eq!(1, provider.links.lock().expect("link mutex").len());
        }
    }

    #[test]
    fn should_search_by_escaped_bound_subject_and_reject_ambiguous_results() {
        let provider = Arc::new(FakeProvider {
            users: vec![
                user(
                    UserStatusType::Confirmed,
                    "first",
                    "quoted\"\\subject",
                    "person@example.test",
                    "true",
                    None,
                ),
                user(
                    UserStatusType::Confirmed,
                    "second",
                    "quoted\"\\subject",
                    "person@example.test",
                    "true",
                    None,
                ),
            ],
            ..FakeProvider::default()
        });

        let result = link(
            &linker(provider.clone()),
            &subject("quoted\"\\subject"),
            "person@example.test",
        );

        assert_eq!(Err(CognitoAccountLinkError::InvalidState), result);
        assert_eq!(
            vec![SearchCall {
                user_pool_id: "pool".to_owned(),
                filter: "sub = \"quoted\\\"\\\\subject\"".to_owned(),
                limit: MAX_SUBJECT_LOOKUP_RESULTS,
            }],
            *provider.searches.lock().expect("search mutex")
        );
        assert!(provider.links.lock().expect("link mutex").is_empty());
    }

    #[test]
    fn should_reject_a_paged_or_missing_subject_lookup() {
        for provider in [
            FakeProvider {
                users: vec![user(
                    UserStatusType::Confirmed,
                    "native-user",
                    "canonical-sub",
                    "person@example.test",
                    "true",
                    None,
                )],
                has_more: true,
                ..FakeProvider::default()
            },
            FakeProvider::default(),
        ] {
            let provider = Arc::new(provider);
            let result = link(
                &linker(provider.clone()),
                &subject("canonical-sub"),
                "person@example.test",
            );
            assert_eq!(Err(CognitoAccountLinkError::InvalidState), result);
            assert!(provider.links.lock().expect("link mutex").is_empty());
        }
    }

    #[test]
    fn should_require_exact_subject_email_and_verified_destination_profile() {
        for destination in [
            user(
                UserStatusType::Confirmed,
                "native-user",
                "other-sub",
                "person@example.test",
                "true",
                None,
            ),
            user(
                UserStatusType::Confirmed,
                "native-user",
                "canonical-sub",
                "Person@example.test",
                "true",
                None,
            ),
            user(
                UserStatusType::Confirmed,
                "native-user",
                "canonical-sub",
                "person@example.test",
                "True",
                None,
            ),
        ] {
            let provider = Arc::new(FakeProvider {
                users: vec![destination],
                ..FakeProvider::default()
            });
            let result = link(
                &linker(provider.clone()),
                &subject("canonical-sub"),
                "person@example.test",
            );
            assert_eq!(Err(CognitoAccountLinkError::InvalidState), result);
            assert!(provider.links.lock().expect("link mutex").is_empty());
        }
    }

    #[test]
    fn should_link_to_native_destination_and_pass_source_fields_through() {
        let provider = Arc::new(FakeProvider {
            users: vec![user(
                UserStatusType::Confirmed,
                "native-user",
                "canonical-sub",
                "person@example.test",
                "true",
                None,
            )],
            ..FakeProvider::default()
        });

        let result = link(
            &linker(provider.clone()),
            &subject("canonical-sub"),
            "person@example.test",
        );

        assert_eq!(Ok(()), result);
        assert_eq!(
            vec![LinkCall {
                user_pool_id: "pool".to_owned(),
                source: ProviderIdentity {
                    provider_name: "Google".to_owned(),
                    attribute_name: "Cognito_Subject".to_owned(),
                    attribute_value: "google-source-id".to_owned(),
                },
                destination: ProviderIdentity {
                    provider_name: "Cognito".to_owned(),
                    attribute_name: "Cognito_Subject".to_owned(),
                    attribute_value: "native-user".to_owned(),
                },
            }],
            *provider.links.lock().expect("link mutex")
        );
    }

    #[test]
    fn should_derive_external_destination_from_primary_identity_after_subject_check() {
        let identities = r#"[
            {"providerType":"OIDC","issuer":null,"dateCreated":"2026-01-01","providerName":"SecondaryIdP","userId":"secondary-id","primary":"false"},
            {"providerName":"CanonicalIdP","userId":"canonical-id","primary":"true"}
        ]"#;
        let provider = Arc::new(FakeProvider {
            users: vec![user(
                UserStatusType::ExternalProvider,
                "external-user",
                "canonical-sub",
                "person@example.test",
                "true",
                Some(identities),
            )],
            ..FakeProvider::default()
        });

        let result = link(
            &linker(provider.clone()),
            &subject("canonical-sub"),
            "person@example.test",
        );

        assert_eq!(Ok(()), result);
        let links = provider.links.lock().expect("link mutex");
        assert_eq!(1, links.len());
        assert_eq!("CanonicalIdP", links[0].destination.provider_name);
        assert_eq!("canonical-id", links[0].destination.attribute_value);
    }

    #[test]
    fn should_never_google_link_to_a_facebook_origin_even_if_its_email_is_now_verified() {
        for (status, identities) in [
            (
                UserStatusType::ExternalProvider,
                Some(r#"[{"providerName":"Facebook","userId":"facebook-id","primary":"true"}]"#),
            ),
            (
                UserStatusType::Confirmed,
                Some(r#"[{"providerName":"Facebook","userId":"facebook-id","primary":"false"}]"#),
            ),
        ] {
            let provider = Arc::new(FakeProvider {
                users: vec![user(
                    status,
                    "facebook-user",
                    "canonical-sub",
                    "person@example.test",
                    "true",
                    identities,
                )],
                ..FakeProvider::default()
            });

            let result = link(
                &linker(provider.clone()),
                &subject("canonical-sub"),
                "person@example.test",
            );

            assert_eq!(Err(CognitoAccountLinkError::InvalidState), result);
            assert!(provider.links.lock().expect("link mutex").is_empty());
        }
    }

    #[test]
    fn should_reject_invalid_or_ambiguous_primary_identity_data() {
        for identities in [
            "not-json",
            "{}",
            "[]",
            r#"[{"providerName":"Google","userId":"id","primary":true},{"providerName":"Other","userId":"other","primary":true}]"#,
        ] {
            let provider = Arc::new(FakeProvider {
                users: vec![user(
                    UserStatusType::ExternalProvider,
                    "external-user",
                    "canonical-sub",
                    "person@example.test",
                    "true",
                    Some(identities),
                )],
                ..FakeProvider::default()
            });
            let result = link(
                &linker(provider.clone()),
                &subject("canonical-sub"),
                "person@example.test",
            );
            assert_eq!(Err(CognitoAccountLinkError::InvalidState), result);
            assert!(provider.links.lock().expect("link mutex").is_empty());
        }
    }

    #[test]
    fn identity_values_remain_opaque() {
        let identity = user_service::ports::CognitoIdentity {
            issuer: CognitoIssuer::try_from("https://issuer.example/pool")
                .unwrap_or_else(|error| panic!("invalid test issuer: {error}")),
            subject: subject("oidc|subject-1"),
        };
        assert_eq!("oidc|subject-1", identity.subject.as_str());
    }
}
