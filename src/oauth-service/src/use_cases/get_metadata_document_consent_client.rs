use crate::error::OAuthServiceError;
use crate::ports::{ClientMetadataDocumentSource, OAuthConnectionGrantReader};
use crate::use_cases::metadata_document_clients::MetadataDocumentClients;
use crate::use_cases::support::authorize_consent_read;
use application::operation_context::{OperationContext, Principal};
use credential_core::metadata_document_client_id::MetadataDocumentClientId;
use oauth_core::client::OAuthClientName;
use oauth_core::metadata_document_client::metadata_document_client_scopes;
use std::collections::HashSet;
use url::Url;
use user_core::access_token::Scope;

#[derive(Debug, Clone, PartialEq)]
pub struct GetMetadataDocumentConsentClientRequest {
    pub client_id: MetadataDocumentClientId,
    pub redirect_uri: Url,
}

/// What the consent page shows about a metadata document client.
///
/// The name, client URI and logo are asserted by the client's own document. The client host
/// and the callback host are what Aura verified: the document is served from the client host
/// and lists the callback.
#[derive(Debug, Clone, PartialEq)]
pub struct MetadataDocumentConsentClientView {
    pub client_id: MetadataDocumentClientId,
    pub name: OAuthClientName,
    pub client_uri: Option<Url>,
    pub logo_uri: Option<Url>,
    pub client_host: String,
    pub redirect_uri_host: String,
    /// Whether every callback the document lists returns to the user's own device.
    pub loopback_redirect_uris_only: bool,
    /// The scopes such a client may request.
    pub scopes: HashSet<Scope>,
    /// The scopes the user already granted this client; a step-up request adds to them.
    pub granted_scopes: HashSet<Scope>,
}

#[async_trait::async_trait]
pub trait GetMetadataDocumentConsentClientUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        request: GetMetadataDocumentConsentClientRequest,
    ) -> Result<MetadataDocumentConsentClientView, OAuthServiceError>;
}

pub struct GetMetadataDocumentConsentClientHandler<S, G> {
    metadata_document_clients: MetadataDocumentClients<S>,
    grants: G,
}

impl<S, G> GetMetadataDocumentConsentClientHandler<S, G> {
    pub fn new(metadata_document_clients: MetadataDocumentClients<S>, grants: G) -> Self {
        Self {
            metadata_document_clients,
            grants,
        }
    }
}

#[async_trait::async_trait]
impl<S, G> GetMetadataDocumentConsentClientUseCase for GetMetadataDocumentConsentClientHandler<S, G>
where
    S: ClientMetadataDocumentSource,
    G: OAuthConnectionGrantReader,
{
    #[tracing::instrument(
        name = "get_oauth_metadata_document_consent_client",
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
        request: GetMetadataDocumentConsentClientRequest,
    ) -> Result<MetadataDocumentConsentClientView, OAuthServiceError> {
        authorize_consent_read(context)?;
        let Principal::User(user_id) = context.principal else {
            return Err(OAuthServiceError::Forbidden);
        };
        let document = self
            .metadata_document_clients
            .resolve(&request.client_id)
            .await?;
        if !document.admits_redirect_uri(&request.redirect_uri) {
            return Err(OAuthServiceError::InvalidRedirectUri);
        }
        let redirect_uri_host = request
            .redirect_uri
            .host_str()
            .ok_or(OAuthServiceError::InvalidRedirectUri)?
            .to_owned();
        let granted_scopes = self
            .grants
            .granted_scopes(user_id, &request.client_id)
            .await?;
        Ok(MetadataDocumentConsentClientView {
            client_host: document.client_id().host().to_owned(),
            redirect_uri_host,
            loopback_redirect_uris_only: document.lists_only_loopback_redirect_uris(),
            name: document.name().clone(),
            client_uri: document.client_uri().cloned(),
            logo_uri: document.logo_uri().cloned(),
            client_id: request.client_id,
            scopes: metadata_document_client_scopes(),
            granted_scopes,
        })
    }
}
