use crate::error::OAuthServiceError;
use crate::ports::{OAuthClientRepository, OAuthClientRepositoryFactory, OAuthClientView};
use crate::use_cases::support::{authorize_oauth_admin, authorize_oauth_client_admin};
use application::idempotency::{self as retry, CommandIntent};
use application::operation_context::OperationContext;
use application::transaction::{Transaction, UnitOfWork};
use credential_core::oauth_client_id::OAuthClientId;
use oauth_core::client::{
    OAuthClient, OAuthClientName, OAuthRedirectUris, RehydratedOAuthClientState,
};
use std::collections::HashSet;
use url::Url;
use user_core::access_token::{HashedRawOAuthClientSecret, RawOAuthClientSecret, Scope};
use user_service::use_cases::queries::check_user_admin::CheckUserAdminUseCase;

#[derive(Debug, Clone, PartialEq)]
pub struct CreateOAuthClientCommand {
    pub name: OAuthClientName,
    pub redirect_uris: HashSet<Url>,
    pub tos_uri: Url,
    pub policy_uri: Url,
    pub client_uri: Url,
    pub logo_uri: Url,
    pub scopes: HashSet<Scope>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CreateOAuthClientResult {
    pub raw_client_secret: RawOAuthClientSecret,
    pub client: OAuthClientView,
}

#[async_trait::async_trait]
pub trait CreateOAuthClientUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        command: CreateOAuthClientCommand,
    ) -> Result<CreateOAuthClientResult, OAuthServiceError>;
}

pub struct CreateOAuthClientHandler<U, C, A> {
    unit_of_work: U,
    clients: C,
    check_user_admin: A,
}
impl<U, C, A> CreateOAuthClientHandler<U, C, A> {
    pub fn new(unit_of_work: U, clients: C, check_user_admin: A) -> Self {
        Self {
            unit_of_work,
            clients,
            check_user_admin,
        }
    }
}

#[async_trait::async_trait]
impl<U, C, A> CreateOAuthClientUseCase for CreateOAuthClientHandler<U, C, A>
where
    U: UnitOfWork,
    C: OAuthClientRepositoryFactory<U::Tx>,
    A: CheckUserAdminUseCase,
{
    #[tracing::instrument(
        name = "create_oauth_client",
        skip_all,
        fields(
            principal_type = context.principal.kind(),
            actor_id = tracing::field::Empty,
            request_id = %context.request_id,
            correlation_id = %context.correlation_id,
        )
    )]
    async fn execute(
        &self,
        context: &OperationContext,
        command: CreateOAuthClientCommand,
    ) -> Result<CreateOAuthClientResult, OAuthServiceError> {
        if let Some(actor_id) = context.principal.actor_id() {
            tracing::Span::current().record("actor_id", tracing::field::display(actor_id));
        }
        authorize_oauth_admin(context)?;
        authorize_oauth_client_admin(context, &self.check_user_admin).await?;
        let redirect_uris = OAuthRedirectUris::try_from(command.redirect_uris)
            .map_err(|error| OAuthServiceError::InvalidClientMetadata(error.to_string()))?;

        let intent = CommandIntent::new(
            context,
            "CREATE_OAUTH_CLIENT",
            "",
            None,
            serde_json::json!({"name":command.name,"redirect_uris":crate::command_receipts::sorted(redirect_uris.iter()),"tos_uri":command.tos_uri,"policy_uri":command.policy_uri,"client_uri":command.client_uri,"logo_uri":command.logo_uri,"scopes":crate::command_receipts::sorted(command.scopes.iter())}),
        );
        let raw_client_secret = RawOAuthClientSecret::new();
        let client = OAuthClient::create(RehydratedOAuthClientState {
            client_id: OAuthClientId::new(),
            hashed_client_secret: HashedRawOAuthClientSecret::from(raw_client_secret.clone()),
            name: command.name,
            redirect_uris,
            tos_uri: command.tos_uri,
            policy_uri: command.policy_uri,
            client_uri: command.client_uri,
            logo_uri: command.logo_uri,
            scopes: command.scopes,
        });

        let mut tx = self.unit_of_work.begin().await?;
        if let Some(intent) = intent.as_ref()
            && let Some(value) = tx.command_receipt(intent).await?
        {
            let issued=<crate::command_receipts::IssuedOAuthClient as application::idempotency::CommandOutcome>::decode(value).map_err(application::idempotency::CommandReceiptError::InvalidPersistedState)?;
            return Err(OAuthServiceError::SecretResultUnavailable {
                client_id: issued.client_id,
            });
        }
        let persisted = self.clients.in_transaction(&mut tx).insert(&client).await?;
        let client_id = persisted.value.client_id();
        retry::complete(
            &mut tx,
            intent.as_ref(),
            &crate::command_receipts::IssuedOAuthClient { client_id },
        )
        .await?;
        tx.commit().await?;

        let client = OAuthClientView::from(persisted);
        tracing::info!(event = "oauth_client.created", actor_id = %context.principal.label(), client_id = %client_id, outcome = "success");
        Ok(CreateOAuthClientResult {
            raw_client_secret,
            client,
        })
    }
}
