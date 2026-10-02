use crate::error::OAuthServiceError;
use crate::ports::{OAuthClientDetailsReader, OAuthClientView};
use application::operation_context::{OperationContext, Principal};
use credential_core::oauth_client_id::OAuthClientId;

#[async_trait::async_trait]
pub trait GetOAuthConsentClientUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        client_id: &OAuthClientId,
    ) -> Result<OAuthClientView, OAuthServiceError>;
}

pub struct GetOAuthConsentClientHandler<R> {
    reader: R,
}

impl<R> GetOAuthConsentClientHandler<R> {
    pub fn new(reader: R) -> Self {
        Self { reader }
    }
}

#[async_trait::async_trait]
impl<R> GetOAuthConsentClientUseCase for GetOAuthConsentClientHandler<R>
where
    R: OAuthClientDetailsReader,
{
    #[tracing::instrument(
        name = "get_oauth_consent_client",
        skip_all,
        fields(
            client_id = %client_id,
            principal_type = context.principal.kind(),
            actor_id = tracing::field::Empty,
            request_id = %context.request_id,
            correlation_id = %context.correlation_id,
        )
    )]
    async fn execute(
        &self,
        context: &OperationContext,
        client_id: &OAuthClientId,
    ) -> Result<OAuthClientView, OAuthServiceError> {
        match &context.principal {
            Principal::User(_) => {}
            Principal::Anonymous => return Err(OAuthServiceError::AuthenticatedActorRequired),
            Principal::DelegatedUser { .. } | Principal::Service(_) | Principal::System => {
                return Err(OAuthServiceError::Forbidden);
            }
        }
        if let Some(actor_id) = context.principal.actor_id() {
            tracing::Span::current().record("actor_id", tracing::field::display(actor_id));
        }

        self.reader
            .find(client_id)
            .await?
            .ok_or(OAuthServiceError::ClientNotFound)
    }
}
