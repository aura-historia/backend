use crate::ports::{ListingSourceAuthorization, SourceAuthorizationError};
use application::{
    error::BoxError,
    operation_context::{
        CredentialCapability, OperationAuthorizationError, OperationContext, Principal,
    },
    transaction::{Transaction, UnitOfWork},
};
use listing_source_core::{ListingIngestionMethod, ListingSourceId};
use listing_source_service::ports::{
    ListingIngestionConfiguration, ListingSourceRepository, ListingSourceRepositoryError,
    ListingSourceRepositoryFactory,
};
use user_core::user_id::UserId;

#[derive(Debug, Clone, PartialEq)]
pub struct PutListingSourceIngestionConfigurationCommand {
    pub listing_source_id: ListingSourceId,
    pub configuration: ListingIngestionConfiguration,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PutListingSourceIngestionConfigurationResult {
    pub created: bool,
}

#[derive(Debug, thiserror::Error)]
pub enum PutListingSourceIngestionConfigurationError {
    #[error("authenticated actor required")]
    AuthenticatedActorRequired,
    #[error("operation not permitted")]
    Forbidden,
    #[error("listing source not found")]
    NotFound,
    #[error("only Shopify and WooCommerce configurations can be replaced here")]
    InvalidConfiguration,
    #[error("concurrent listing source update")]
    ConcurrencyConflict,
    #[error("listing source Shopify domain conflict")]
    ShopifyDomainConflict {
        #[source]
        source: BoxError,
    },
    #[error("temporary listing source configuration failure")]
    TemporarilyUnavailable {
        #[source]
        source: BoxError,
    },
    #[error("invalid persisted listing source state")]
    InvalidPersistedState {
        #[source]
        source: BoxError,
    },
    #[error("internal listing source configuration failure")]
    Internal {
        #[source]
        source: BoxError,
    },
    #[error("failed to begin listing source configuration transaction")]
    BeginTransactionFailed,
    #[error("failed to commit listing source configuration transaction")]
    CommitTransactionFailed,
}

#[async_trait::async_trait]
pub trait PutListingSourceIngestionConfigurationUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        command: PutListingSourceIngestionConfigurationCommand,
    ) -> Result<
        PutListingSourceIngestionConfigurationResult,
        PutListingSourceIngestionConfigurationError,
    >;
}

pub struct PutListingSourceIngestionConfigurationHandler<U, S, A> {
    unit_of_work: U,
    sources: S,
    authorization: A,
}

impl<U, S, A> PutListingSourceIngestionConfigurationHandler<U, S, A> {
    pub fn new(unit_of_work: U, sources: S, authorization: A) -> Self {
        Self {
            unit_of_work,
            sources,
            authorization,
        }
    }
}

#[async_trait::async_trait]
impl<U, S, A> PutListingSourceIngestionConfigurationUseCase
    for PutListingSourceIngestionConfigurationHandler<U, S, A>
where
    U: UnitOfWork,
    S: ListingSourceRepositoryFactory<U::Tx>,
    A: ListingSourceAuthorization,
{
    #[tracing::instrument(
        name = "put_listing_source_ingestion_configuration",
        skip_all,
        fields(
            listing_source_id = %command.listing_source_id,
            principal_type = context.principal.kind(),
            actor_id = tracing::field::Empty,
            request_id = %context.request_id,
            correlation_id = %context.correlation_id,
            created = tracing::field::Empty,
            outcome = tracing::field::Empty,
        )
    )]
    async fn execute(
        &self,
        context: &OperationContext,
        command: PutListingSourceIngestionConfigurationCommand,
    ) -> Result<
        PutListingSourceIngestionConfigurationResult,
        PutListingSourceIngestionConfigurationError,
    > {
        let user_id = actor(context)?;
        tracing::Span::current().record(
            "actor_id",
            tracing::field::display(context.principal.label()),
        );
        let authorized = self
            .authorization
            .can_write_source(user_id, command.listing_source_id)
            .await
            .map_err(map_authorization)?;
        if !authorized {
            return Err(PutListingSourceIngestionConfigurationError::Forbidden);
        }

        let method = command.configuration.method();
        if !matches!(
            method,
            ListingIngestionMethod::Shopify | ListingIngestionMethod::Woocommerce
        ) {
            return Err(PutListingSourceIngestionConfigurationError::InvalidConfiguration);
        }

        let mut tx = self
            .unit_of_work
            .begin()
            .await
            .map_err(|_| PutListingSourceIngestionConfigurationError::BeginTransactionFailed)?;
        let stored = self
            .sources
            .in_transaction(&mut tx)
            .find_by_id(command.listing_source_id)
            .await
            .map_err(map_repository)?
            .ok_or(PutListingSourceIngestionConfigurationError::NotFound)?;

        let was_configured = stored
            .configuration
            .0
            .iter()
            .any(|configuration| configuration.method() == method);
        let mut source = stored.source;
        let mut configuration = stored.configuration;
        let configuration_changed = configuration
            .replace_provider_configuration(command.configuration)
            .map_err(|_| PutListingSourceIngestionConfigurationError::InvalidConfiguration)?;
        let mut methods = source.ingestion_methods().clone();
        methods.insert(method);
        let methods_changed = source.replace_ingestion_methods(methods).changed();
        configuration
            .validate_for(&source)
            .map_err(|_| PutListingSourceIngestionConfigurationError::InvalidConfiguration)?;

        if configuration_changed || methods_changed {
            self.sources
                .in_transaction(&mut tx)
                .update(&source, &configuration, stored.version)
                .await
                .map_err(map_repository)?;
        }
        tx.commit()
            .await
            .map_err(|_| PutListingSourceIngestionConfigurationError::CommitTransactionFailed)?;
        let created = !was_configured;
        tracing::Span::current().record("created", created);
        tracing::Span::current().record("outcome", "success");
        Ok(PutListingSourceIngestionConfigurationResult { created })
    }
}

fn actor(
    context: &OperationContext,
) -> Result<UserId, PutListingSourceIngestionConfigurationError> {
    context
        .require()
        .credential_capability(CredentialCapability::ListingSourcesWrite)
        .any_user()
        .authorize::<OperationAuthorizationError>()
        .map_err(|error| match error {
            OperationAuthorizationError::AuthenticationRequired(_) => {
                PutListingSourceIngestionConfigurationError::AuthenticatedActorRequired
            }
            OperationAuthorizationError::Forbidden
            | OperationAuthorizationError::InsufficientCapability { .. } => {
                PutListingSourceIngestionConfigurationError::Forbidden
            }
        })?;
    match context.principal {
        Principal::User(user_id) | Principal::DelegatedUser { user_id, .. } => Ok(user_id),
        _ => Err(PutListingSourceIngestionConfigurationError::Forbidden),
    }
}

fn map_authorization(
    error: SourceAuthorizationError,
) -> PutListingSourceIngestionConfigurationError {
    match error {
        SourceAuthorizationError::TemporarilyUnavailable { source } => {
            PutListingSourceIngestionConfigurationError::TemporarilyUnavailable { source }
        }
        SourceAuthorizationError::InvalidReadModel { source }
        | SourceAuthorizationError::Internal { source } => {
            PutListingSourceIngestionConfigurationError::Internal { source }
        }
    }
}

fn map_repository(
    error: ListingSourceRepositoryError,
) -> PutListingSourceIngestionConfigurationError {
    match error {
        ListingSourceRepositoryError::ConcurrencyConflict => {
            PutListingSourceIngestionConfigurationError::ConcurrencyConflict
        }
        ListingSourceRepositoryError::ShopifyDomainConflict { source } => {
            PutListingSourceIngestionConfigurationError::ShopifyDomainConflict { source }
        }
        ListingSourceRepositoryError::TemporarilyUnavailable { source } => {
            PutListingSourceIngestionConfigurationError::TemporarilyUnavailable { source }
        }
        ListingSourceRepositoryError::InvalidPersistedState { source } => {
            PutListingSourceIngestionConfigurationError::InvalidPersistedState { source }
        }
        ListingSourceRepositoryError::SlugConflict { source }
        | ListingSourceRepositoryError::Internal { source } => {
            PutListingSourceIngestionConfigurationError::Internal { source }
        }
    }
}

impl From<OperationAuthorizationError> for PutListingSourceIngestionConfigurationError {
    fn from(error: OperationAuthorizationError) -> Self {
        match error {
            OperationAuthorizationError::AuthenticationRequired(_) => {
                Self::AuthenticatedActorRequired
            }
            OperationAuthorizationError::Forbidden
            | OperationAuthorizationError::InsufficientCapability { .. } => Self::Forbidden,
        }
    }
}
