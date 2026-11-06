use crate::error::OAuthServiceError;
use crate::ports::{
    AuthorizationCodeRepository, AuthorizationCodeRepositoryFactory, ClientMetadataDocumentSource,
    OAuthClientRepository, OAuthClientRepositoryFactory, OAuthConnectionRepository,
    OAuthConnectionRepositoryFactory,
};
use crate::use_cases::metadata_document_clients::MetadataDocumentClients;
use crate::use_cases::support::{AUTHORIZATION_CODE_TTL, append_query_params};
use application::operation_context::{CredentialCapability, OperationContext, Principal};
use application::transaction::{Transaction, UnitOfWork};
use credential_core::metadata_document_client_id::MetadataDocumentClientId;
use credential_core::oauth_client_id::OAuthClientId;
use domain_primitives::string_newtype;
use oauth_core::authorization_code::{
    AuthorizationCode, CodeChallengeMethod, OAuthAuthorizationCode, OAuthCodeChallenge,
    RehydratedAuthorizationCodeState,
};
use oauth_core::client_identifier::OAuthClientIdentifier;
use oauth_core::connection::{OAuthConnection, OAuthConnectionApproval};
use oauth_core::metadata_document_client::{
    ClientMetadataDocument, metadata_document_client_scopes,
};
use oauth_core::resource::{AuraResource, AuraResourcePolicy};
use std::collections::HashSet;
use time::OffsetDateTime;
use user_core::access_token::Scope;
use user_core::user_id::UserId;

string_newtype!(OAuthState);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OAuthResponseType {
    Code,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AuthorizeRequest {
    pub response_type: OAuthResponseType,
    pub client_id: OAuthClientIdentifier,
    pub redirect_uri: url::Url,
    pub resource: Option<String>,
    pub scope: HashSet<Scope>,
    /// Whether `scope` also listed `offline_access`, which asks for a refresh token.
    pub offline_access: bool,
    pub state: Option<OAuthState>,
    pub code_challenge: OAuthCodeChallenge,
    pub code_challenge_method: CodeChallengeMethod,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AuthorizeResponse {
    pub redirect_to: String,
}

#[async_trait::async_trait]
pub trait AuthorizeUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        request: AuthorizeRequest,
    ) -> Result<AuthorizeResponse, OAuthServiceError>;
}

pub struct AuthorizeHandler<U, C, A, N, S> {
    unit_of_work: U,
    clients: C,
    codes: A,
    connections: N,
    metadata_document_clients: MetadataDocumentClients<S>,
    resource_policy: AuraResourcePolicy,
}
impl<U, C, A, N, S> AuthorizeHandler<U, C, A, N, S> {
    pub fn new(
        unit_of_work: U,
        clients: C,
        codes: A,
        connections: N,
        metadata_document_clients: MetadataDocumentClients<S>,
        resource_policy: AuraResourcePolicy,
    ) -> Self {
        Self {
            unit_of_work,
            clients,
            codes,
            connections,
            metadata_document_clients,
            resource_policy,
        }
    }
}
#[async_trait::async_trait]
impl<U, C, A, N, S> AuthorizeUseCase for AuthorizeHandler<U, C, A, N, S>
where
    U: UnitOfWork,
    C: OAuthClientRepositoryFactory<U::Tx>,
    A: AuthorizationCodeRepositoryFactory<U::Tx>,
    N: OAuthConnectionRepositoryFactory<U::Tx>,
    S: ClientMetadataDocumentSource,
{
    #[tracing::instrument(
        name = "authorize_oauth_request",
        skip_all,
        fields(
            client_id = %request.client_id,
            principal_type = context.principal.kind(),
            actor_id = tracing::field::Empty,
            request_id = %context.request_id,
            correlation_id = %context.correlation_id,
        )
    )]
    async fn execute(
        &self,
        context: &OperationContext,
        request: AuthorizeRequest,
    ) -> Result<AuthorizeResponse, OAuthServiceError> {
        if let Some(actor_id) = context.principal.actor_id() {
            tracing::Span::current().record("actor_id", tracing::field::display(actor_id));
        }
        let user_id = authorized_user_id(context, &request.scope)?;
        let resource = self
            .resource_policy
            .resolve(request.resource.as_deref())
            .map_err(|_| OAuthServiceError::InvalidResource)?;
        let (tx, issues_refresh_token) = match &request.client_id {
            OAuthClientIdentifier::Registered(client_id) => {
                let mut tx = self.unit_of_work.begin().await?;
                self.ensure_registered_client_allows(&mut tx, *client_id, &request)
                    .await?;
                (tx, false)
            }
            OAuthClientIdentifier::MetadataDocument(client_id) => {
                // Fetch before the transaction so outbound I/O never holds a connection.
                let document = self
                    .ensure_metadata_document_client_allows(client_id, &request)
                    .await?;
                let mut tx = self.unit_of_work.begin().await?;
                self.record_approval(&mut tx, user_id, &document, &request.scope)
                    .await?;
                let issues_refresh_token =
                    request.offline_access || document.declares_refresh_token_grant();
                (tx, issues_refresh_token)
            }
        };
        self.issue_code(tx, user_id, resource, issues_refresh_token, request)
            .await
    }
}

impl<U, C, A, N, S> AuthorizeHandler<U, C, A, N, S>
where
    U: UnitOfWork,
    C: OAuthClientRepositoryFactory<U::Tx>,
    A: AuthorizationCodeRepositoryFactory<U::Tx>,
    N: OAuthConnectionRepositoryFactory<U::Tx>,
    S: ClientMetadataDocumentSource,
{
    async fn ensure_registered_client_allows(
        &self,
        tx: &mut U::Tx,
        client_id: OAuthClientId,
        request: &AuthorizeRequest,
    ) -> Result<(), OAuthServiceError> {
        let client = self
            .clients
            .in_transaction(tx)
            .find_by_id(client_id)
            .await?
            .ok_or(OAuthServiceError::ClientNotFound)?
            .value;
        if !client
            .redirect_uris()
            .as_set()
            .contains(&request.redirect_uri)
        {
            return Err(OAuthServiceError::InvalidRedirectUri);
        }
        // Registered clients never receive refresh tokens.
        if request.offline_access || !request.scope.is_subset(client.scopes()) {
            return Err(OAuthServiceError::InvalidScope);
        }
        Ok(())
    }

    async fn ensure_metadata_document_client_allows(
        &self,
        client_id: &MetadataDocumentClientId,
        request: &AuthorizeRequest,
    ) -> Result<ClientMetadataDocument, OAuthServiceError> {
        let document = self.metadata_document_clients.resolve(client_id).await?;
        if !document.admits_redirect_uri(&request.redirect_uri) {
            return Err(OAuthServiceError::InvalidRedirectUri);
        }
        if !request.scope.is_subset(&metadata_document_client_scopes()) {
            return Err(OAuthServiceError::InvalidScope);
        }
        Ok(document)
    }

    /// Starts the user's connection to the client, or adds the approved scopes to it.
    async fn record_approval(
        &self,
        tx: &mut U::Tx,
        user_id: UserId,
        document: &ClientMetadataDocument,
        scopes: &HashSet<Scope>,
    ) -> Result<(), OAuthServiceError> {
        let approval = OAuthConnectionApproval {
            user_id,
            client_id: document.client_id(),
            client_name: document.name(),
            scopes,
            approved_at: OffsetDateTime::now_utc(),
        };
        let mut connections = self.connections.in_transaction(tx);
        match connections
            .find_by_user_and_client_for_update(user_id, document.client_id())
            .await?
        {
            Some(mut connection) => {
                connection.approve_again(approval);
                connections.update(&connection).await?;
            }
            None => {
                connections
                    .insert(&OAuthConnection::start(approval))
                    .await?
            }
        }
        Ok(())
    }

    /// Persists a code bound to the client, callback, resource and PKCE challenge, and
    /// returns the success response (RFC 9207 `iss` included) for the validated callback.
    async fn issue_code(
        &self,
        mut tx: U::Tx,
        user_id: UserId,
        resource: AuraResource,
        issues_refresh_token: bool,
        request: AuthorizeRequest,
    ) -> Result<AuthorizeResponse, OAuthServiceError> {
        let now = OffsetDateTime::now_utc();
        let code = AuthorizationCode::create(RehydratedAuthorizationCodeState {
            code: OAuthAuthorizationCode::new(),
            client_id: request.client_id,
            user_id,
            redirect_uri: request.redirect_uri.clone(),
            resource: Some(resource),
            scopes: request.scope,
            code_challenge: request.code_challenge,
            code_challenge_method: request.code_challenge_method,
            issues_refresh_token,
            expires: now + AUTHORIZATION_CODE_TTL,
        });
        self.codes
            .in_transaction(&mut tx)
            .insert(code.clone())
            .await?;
        let code_value = code.code().to_string();
        let mut params = vec![("code", code_value.as_str())];
        if let Some(state) = &request.state {
            params.push(("state", state.as_ref()));
        }
        params.push(("iss", self.resource_policy.issuer()));
        let response = AuthorizeResponse {
            redirect_to: append_query_params(&request.redirect_uri, &params),
        };
        tx.commit().await?;
        Ok(response)
    }
}

/// Approval requires first-party authority. The operation context does not carry a
/// delegation's lifetime, so a delegate could approve an OAuth token that outlives its own
/// grant (`docs/arch.md` §15.6). A delegate that lacks `access-tokens:write` or a requested
/// scope keeps the generic denial.
fn authorized_user_id(
    context: &OperationContext,
    requested_scopes: &HashSet<Scope>,
) -> Result<UserId, OAuthServiceError> {
    match &context.principal {
        Principal::User(user_id) => Ok(*user_id),
        Principal::DelegatedUser { capabilities, .. } => {
            if !capabilities.contains(&CredentialCapability::AccessTokensWrite)
                || requested_scopes
                    .iter()
                    .any(|scope| !capabilities.contains(scope))
            {
                return Err(OAuthServiceError::Forbidden);
            }
            let error = OAuthServiceError::DelegatedAuthorizationForbidden;
            tracing::warn!(
                event = "oauth_authorization_code.created",
                actor_id = %context.principal.label(),
                error_category = %error,
                outcome = "failure",
            );
            Err(error)
        }
        Principal::Anonymous => Err(OAuthServiceError::AuthenticatedActorRequired),
        Principal::Service(_) | Principal::System => Err(OAuthServiceError::Forbidden),
    }
}
