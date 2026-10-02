use std::sync::Arc;

use async_trait::async_trait;
use aws_sdk_cognitoidentityprovider::{
    Client,
    error::ProvideErrorMetadata,
    types::{ProviderUserIdentifierType, UserStatusType, UserType},
};
use serde_json::Value;

const MAX_EMAIL_LOOKUP_RESULTS: i32 = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CognitoAccountLinkOutcome {
    NoExistingAccount,
    Linked,
}

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
        verified_email: &str,
    ) -> Result<CognitoAccountLinkOutcome, CognitoAccountLinkError> {
        let lookup = self
            .provider
            .find_users_by_email(
                user_pool_id,
                &email_filter(verified_email),
                MAX_EMAIL_LOOKUP_RESULTS,
            )
            .await?;
        if lookup.has_more {
            return Err(CognitoAccountLinkError::InvalidState);
        }

        let [user] = lookup.users.as_slice() else {
            return if lookup.users.is_empty() {
                Ok(CognitoAccountLinkOutcome::NoExistingAccount)
            } else {
                Err(CognitoAccountLinkError::InvalidState)
            };
        };

        require_verified_email(user, verified_email)?;
        let destination = destination_for(user)?;
        let source = ProviderIdentity {
            provider_name: source_provider_name.to_owned(),
            attribute_name: source_provider_attribute_name.to_owned(),
            attribute_value: source_subject.to_owned(),
        };

        self.provider
            .link_provider(user_pool_id, &source, &destination)
            .await?;
        Ok(CognitoAccountLinkOutcome::Linked)
    }
}

#[derive(Default)]
struct EmailLookup {
    users: Vec<UserType>,
    has_more: bool,
}

#[async_trait]
trait CognitoAccountLinkProvider: Send + Sync {
    async fn find_users_by_email(
        &self,
        user_pool_id: &str,
        filter: &str,
        limit: i32,
    ) -> Result<EmailLookup, CognitoAccountLinkError>;

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
    async fn find_users_by_email(
        &self,
        user_pool_id: &str,
        filter: &str,
        limit: i32,
    ) -> Result<EmailLookup, CognitoAccountLinkError> {
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

        Ok(EmailLookup {
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

fn email_filter(email: &str) -> String {
    let mut escaped = String::with_capacity(email.len());
    for character in email.chars() {
        if matches!(character, '\\' | '"') {
            escaped.push('\\');
        }
        escaped.push(character);
    }
    format!("email = \"{escaped}\"")
}

fn require_verified_email(
    user: &UserType,
    requested_email: &str,
) -> Result<(), CognitoAccountLinkError> {
    let email = attribute(user, "email")?
        .and_then(|attribute| attribute.value())
        .ok_or(CognitoAccountLinkError::InvalidState)?;
    if !email.eq_ignore_ascii_case(requested_email) {
        return Err(CognitoAccountLinkError::InvalidState);
    }

    let verified = attribute(user, "email_verified")?
        .and_then(|attribute| attribute.value())
        .is_some_and(|value| value.eq_ignore_ascii_case("true"));
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
        Value::String(value) if value.eq_ignore_ascii_case("true") => Some(true),
        Value::String(value) if value.eq_ignore_ascii_case("false") => Some(false),
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
        searches: Mutex<Vec<SearchCall>>,
        links: Mutex<Vec<LinkCall>>,
    }

    #[async_trait]
    impl CognitoAccountLinkProvider for FakeProvider {
        async fn find_users_by_email(
            &self,
            user_pool_id: &str,
            filter: &str,
            limit: i32,
        ) -> Result<EmailLookup, CognitoAccountLinkError> {
            self.searches
                .lock()
                .expect("search mutex")
                .push(SearchCall {
                    user_pool_id: user_pool_id.to_owned(),
                    filter: filter.to_owned(),
                    limit,
                });
            Ok(EmailLookup {
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
            Ok(())
        }
    }

    fn linker(provider: Arc<FakeProvider>) -> CognitoAccountLinker {
        CognitoAccountLinker { provider }
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
        email: &str,
        email_verified: &str,
        identities: Option<&str>,
    ) -> UserType {
        user_with_status(Some(status), username, email, email_verified, identities)
    }

    fn user_with_status(
        status: Option<UserStatusType>,
        username: &str,
        email: &str,
        email_verified: &str,
        identities: Option<&str>,
    ) -> UserType {
        let mut attributes = vec![
            attribute_type("email", email),
            attribute_type("email_verified", email_verified),
        ];
        if let Some(identities) = identities {
            attributes.push(attribute_type("identities", identities));
        }
        let mut builder = UserType::builder().username(username);
        if let Some(status) = status {
            builder = builder.user_status(status);
        }
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

    #[test]
    fn should_escape_email_filter_and_stop_at_bounded_ambiguity() {
        let email = r#"a"\b@example.com"#;
        let provider = Arc::new(FakeProvider {
            users: vec![
                user(UserStatusType::Confirmed, "first", email, "true", None),
                user(UserStatusType::Confirmed, "second", email, "true", None),
            ],
            ..FakeProvider::default()
        });

        let result = block_on(linker(provider.clone()).link_account(
            "pool",
            "NewProvider",
            "subject",
            "new-subject",
            email,
        ));

        assert_eq!(Err(CognitoAccountLinkError::InvalidState), result);
        assert_eq!(
            vec![SearchCall {
                user_pool_id: "pool".to_owned(),
                filter: r#"email = "a\"\\b@example.com""#.to_owned(),
                limit: MAX_EMAIL_LOOKUP_RESULTS,
            }],
            *provider.searches.lock().expect("search mutex")
        );
        assert!(provider.links.lock().expect("link mutex").is_empty());
    }

    #[test]
    fn should_reject_a_lookup_page_with_more_matching_users() {
        let provider = Arc::new(FakeProvider {
            users: vec![user(
                UserStatusType::Confirmed,
                "first",
                "person@example.com",
                "true",
                None,
            )],
            has_more: true,
            ..FakeProvider::default()
        });

        let result = block_on(linker(provider.clone()).link_account(
            "pool",
            "NewProvider",
            "subject",
            "new-subject",
            "person@example.com",
        ));

        assert_eq!(Err(CognitoAccountLinkError::InvalidState), result);
        assert!(provider.links.lock().expect("link mutex").is_empty());
    }

    #[test]
    fn should_return_no_existing_account_without_linking() {
        let provider = Arc::new(FakeProvider::default());

        let result = block_on(linker(provider.clone()).link_account(
            "pool",
            "NewProvider",
            "subject",
            "new-subject",
            "person@example.com",
        ));

        assert_eq!(Ok(CognitoAccountLinkOutcome::NoExistingAccount), result);
        assert!(provider.links.lock().expect("link mutex").is_empty());
    }

    #[test]
    fn should_require_a_matching_verified_destination_email() {
        for destination in [
            user(
                UserStatusType::Confirmed,
                "unverified",
                "person@example.com",
                "false",
                None,
            ),
            user(
                UserStatusType::Confirmed,
                "different-email",
                "other@example.com",
                "true",
                None,
            ),
        ] {
            let provider = Arc::new(FakeProvider {
                users: vec![destination],
                ..FakeProvider::default()
            });
            let result = block_on(linker(provider.clone()).link_account(
                "pool",
                "NewProvider",
                "subject",
                "new-subject",
                "person@example.com",
            ));

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
                "person@example.com",
                "true",
                None,
            )],
            ..FakeProvider::default()
        });

        let result = block_on(linker(provider.clone()).link_account(
            "pool",
            "ExternalProvider",
            "custom-subject-claim",
            "provider-subject-123",
            "person@example.com",
        ));

        assert_eq!(Ok(CognitoAccountLinkOutcome::Linked), result);
        assert_eq!(
            vec![LinkCall {
                user_pool_id: "pool".to_owned(),
                source: ProviderIdentity {
                    provider_name: "ExternalProvider".to_owned(),
                    attribute_name: "custom-subject-claim".to_owned(),
                    attribute_value: "provider-subject-123".to_owned(),
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
    fn should_choose_the_primary_external_identity_from_structured_json() {
        let identities = r#"[
            {"providerType":"OIDC","issuer":null,"dateCreated":"2026-01-01","providerName":"SecondaryIdP","userId":"secondary-id","primary":"false"},
            {"providerName":"Canonical\u0049dP","userId":"canonical-id","primary":"true","metadata":{"nested":[true,null,1.25]}}
        ]"#;
        let provider = Arc::new(FakeProvider {
            users: vec![user(
                UserStatusType::ExternalProvider,
                "external-user",
                "person@example.com",
                "true",
                Some(identities),
            )],
            ..FakeProvider::default()
        });

        let result = block_on(linker(provider.clone()).link_account(
            "pool",
            "NewProvider",
            "subject",
            "source-id",
            "person@example.com",
        ));

        assert_eq!(Ok(CognitoAccountLinkOutcome::Linked), result);
        let links = provider.links.lock().expect("link mutex");
        assert_eq!(1, links.len());
        assert_eq!(
            ProviderIdentity {
                provider_name: "CanonicalIdP".to_owned(),
                attribute_name: "Cognito_Subject".to_owned(),
                attribute_value: "canonical-id".to_owned(),
            },
            links[0].destination
        );
    }

    #[test]
    fn should_require_identities_for_external_provider_status() {
        for identities in [None, Some("[]")] {
            let external = user(
                UserStatusType::ExternalProvider,
                "external-user",
                "person@example.com",
                "true",
                identities,
            );

            assert_eq!(
                Err(CognitoAccountLinkError::InvalidState),
                destination_for(&external)
            );
        }
    }

    #[test]
    fn should_reject_missing_or_unknown_status() {
        let missing_status =
            user_with_status(None, "missing-status", "person@example.com", "true", None);
        let ambiguous_status = user_with_status(
            Some(UserStatusType::UnknownValue),
            "unknown-status",
            "person@example.com",
            "true",
            None,
        );

        for candidate in [missing_status, ambiguous_status] {
            assert_eq!(
                Err(CognitoAccountLinkError::InvalidState),
                destination_for(&candidate)
            );
        }
    }

    #[test]
    fn should_link_an_additional_provider_to_native_user_with_existing_linked_identities() {
        let identities = r#"[
            {"providerName":"Google","userId":"google-subject","primary":"true"},
            {"providerName":"ExistingOidc","userId":"existing-subject","primary":"false"}
        ]"#;
        let provider = Arc::new(FakeProvider {
            users: vec![user(
                UserStatusType::Confirmed,
                "native-user",
                "person@example.com",
                "true",
                Some(identities),
            )],
            ..FakeProvider::default()
        });

        let result = block_on(linker(provider.clone()).link_account(
            "pool",
            "NewProvider",
            "Cognito_Subject",
            "new-provider-subject",
            "person@example.com",
        ));

        assert_eq!(Ok(CognitoAccountLinkOutcome::Linked), result);
        assert_eq!(
            vec![LinkCall {
                user_pool_id: "pool".to_owned(),
                source: ProviderIdentity {
                    provider_name: "NewProvider".to_owned(),
                    attribute_name: "Cognito_Subject".to_owned(),
                    attribute_value: "new-provider-subject".to_owned(),
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
    fn should_reject_malformed_or_ambiguous_external_identity_state() {
        for identities in [
            "not-json",
            r#"[{"providerName":"IdP","userId":"id","primary":"false"}]"#,
            r#"[
                {"providerName":"First","userId":"first-id","primary":"true"},
                {"providerName":"Second","userId":"second-id","primary":"true"}
            ]"#,
        ] {
            assert_eq!(
                Err(CognitoAccountLinkError::InvalidState),
                primary_external_identity(identities).map(|identity| identity.unwrap_or_else(
                    || ProviderIdentity {
                        provider_name: "Cognito".to_owned(),
                        attribute_name: "Cognito_Subject".to_owned(),
                        attribute_value: "native".to_owned(),
                    }
                ))
            );
        }
    }

    #[test]
    fn should_classify_retryable_and_invalid_state_errors_without_error_text() {
        for code in [
            None,
            Some("InternalErrorException"),
            Some("TooManyRequestsException"),
            Some("LimitExceededException"),
            Some("ConcurrentModificationException"),
        ] {
            assert_eq!(
                CognitoAccountLinkError::TemporarilyUnavailable,
                classify_cognito_error_code(code)
            );
        }

        let invalid = classify_cognito_error_code(Some("InvalidParameterException"));
        assert_eq!(CognitoAccountLinkError::InvalidState, invalid);
        assert_eq!(
            "Cognito account linking encountered invalid account state",
            invalid.to_string()
        );
        assert!(!format!("{invalid:?}").contains("InvalidParameterException"));
    }
}
