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
            .find_by_id_for_update(command.listing_source_id)
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

#[cfg(test)]
mod tests {
    use super::*;
    use application::{
        error::static_error,
        operation_context::{CorrelationId, RequestId},
        transaction::TransactionError,
    };
    use listing_source_core::{
        ListingSource, ListingSourceName, ListingSourcePresentation, NewListingSource,
        ReferralConfiguration,
    };
    use listing_source_service::ports::{
        ListingIngestionConfiguration, ListingSourceDeletionBlocker,
        ListingSourceIngestionConfigurations, ListingSourceStorageVersion, StoredListingSource,
    };
    use party_core::party_id::PartyId;
    use std::sync::{Arc, Mutex, MutexGuard};
    use time::OffsetDateTime;

    #[derive(Default)]
    struct State {
        stored: Option<StoredListingSource>,
        ordinary_reads: usize,
        locked_reads: usize,
        updates: usize,
        begins: usize,
        commits: usize,
    }

    struct Shared(Arc<Mutex<State>>);
    struct Tx(Arc<Mutex<State>>);
    struct Sources(Arc<Mutex<State>>);
    struct Repository(Arc<Mutex<State>>);
    struct Authorization {
        allowed: bool,
    }

    fn lock(state: &Arc<Mutex<State>>) -> MutexGuard<'_, State> {
        state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn stored_source(id: ListingSourceId) -> StoredListingSource {
        StoredListingSource {
            source: ListingSource::create(NewListingSource {
                id,
                name: ListingSourceName::try_from("Test ListingSource")
                    .unwrap_or_else(|error| panic!("invalid test ListingSource name: {error}")),
                operator_party_id: PartyId::new(),
                ingestion_methods: Default::default(),
                presentation: ListingSourcePresentation::default(),
                referral_configuration: None::<ReferralConfiguration>,
            }),
            configuration: ListingSourceIngestionConfigurations::default(),
            version: ListingSourceStorageVersion::INITIAL,
            created: OffsetDateTime::UNIX_EPOCH,
            updated: OffsetDateTime::UNIX_EPOCH,
        }
    }

    #[async_trait::async_trait]
    impl Transaction for Tx {
        async fn commit(self) -> Result<(), TransactionError> {
            lock(&self.0).commits += 1;
            Ok(())
        }
    }

    #[async_trait::async_trait]
    impl UnitOfWork for Shared {
        type Tx = Tx;

        async fn begin(&self) -> Result<Self::Tx, TransactionError> {
            lock(&self.0).begins += 1;
            Ok(Tx(Arc::clone(&self.0)))
        }
    }

    impl ListingSourceRepositoryFactory<Tx> for Sources {
        fn in_transaction<'tx>(&'tx self, _tx: &'tx mut Tx) -> impl ListingSourceRepository + 'tx {
            Repository(Arc::clone(&self.0))
        }
    }

    #[async_trait::async_trait]
    impl ListingSourceRepository for Repository {
        async fn find_by_id(
            &mut self,
            _id: ListingSourceId,
        ) -> Result<Option<StoredListingSource>, ListingSourceRepositoryError> {
            lock(&self.0).ordinary_reads += 1;
            Ok(lock(&self.0).stored.clone())
        }

        async fn find_by_slug(
            &mut self,
            _slug: &listing_source_core::ListingSourceSlugId,
        ) -> Result<Option<StoredListingSource>, ListingSourceRepositoryError> {
            Ok(None)
        }

        async fn find_by_id_for_update(
            &mut self,
            _id: ListingSourceId,
        ) -> Result<Option<StoredListingSource>, ListingSourceRepositoryError> {
            let mut state = lock(&self.0);
            state.locked_reads += 1;
            Ok(state.stored.clone())
        }

        async fn find_deletion_blocker(
            &mut self,
            _id: ListingSourceId,
        ) -> Result<Option<ListingSourceDeletionBlocker>, ListingSourceRepositoryError> {
            Ok(None)
        }

        async fn delete_unused(
            &mut self,
            _id: ListingSourceId,
            _expected: ListingSourceStorageVersion,
        ) -> Result<(), ListingSourceRepositoryError> {
            Err(ListingSourceRepositoryError::Internal {
                source: static_error("unexpected delete"),
            })
        }

        async fn insert(
            &mut self,
            _source: &ListingSource,
            _configuration: &ListingSourceIngestionConfigurations,
        ) -> Result<StoredListingSource, ListingSourceRepositoryError> {
            Err(ListingSourceRepositoryError::Internal {
                source: static_error("unexpected insert"),
            })
        }

        async fn update(
            &mut self,
            source: &ListingSource,
            configuration: &ListingSourceIngestionConfigurations,
            expected: ListingSourceStorageVersion,
        ) -> Result<StoredListingSource, ListingSourceRepositoryError> {
            let mut state = lock(&self.0);
            state.updates += 1;
            let stored = StoredListingSource {
                source: source.clone(),
                configuration: configuration.clone(),
                version: expected,
                created: OffsetDateTime::UNIX_EPOCH,
                updated: OffsetDateTime::UNIX_EPOCH,
            };
            state.stored = Some(stored.clone());
            Ok(stored)
        }
    }

    #[async_trait::async_trait]
    impl ListingSourceAuthorization for Authorization {
        async fn can_write_source(
            &self,
            _user_id: UserId,
            _listing_source_id: ListingSourceId,
        ) -> Result<bool, SourceAuthorizationError> {
            Ok(self.allowed)
        }

        async fn list_sources_user_administers(
            &self,
            _user_id: UserId,
        ) -> Result<Vec<crate::ports::AdministeredListingSource>, SourceAuthorizationError>
        {
            Ok(Vec::new())
        }
    }

    fn context(principal: Principal) -> OperationContext {
        OperationContext {
            principal,
            request_id: RequestId::new("request"),
            correlation_id: CorrelationId::new("correlation"),
        }
    }

    fn command(id: ListingSourceId) -> PutListingSourceIngestionConfigurationCommand {
        PutListingSourceIngestionConfigurationCommand {
            listing_source_id: id,
            configuration: ListingIngestionConfiguration::Woocommerce {
                webhook_secret: listing_source_core::WoocommerceWebhookSecret::try_from(
                    "test-secret",
                )
                .unwrap_or_else(|error| panic!("invalid test secret: {error}")),
                currency: None,
                language: None,
            },
        }
    }

    #[tokio::test]
    async fn should_use_locked_post_read_state_to_decide_created_and_skip_identical_write() {
        let id = ListingSourceId::new();
        let initial = State {
            stored: Some(stored_source(id)),
            ..State::default()
        };
        let shared = Shared(Arc::new(Mutex::new(initial)));
        let sources = Sources(Arc::clone(&shared.0));
        let handler = PutListingSourceIngestionConfigurationHandler::new(
            shared,
            sources,
            Authorization { allowed: true },
        );
        let user_id = UserId::new();
        let context = context(Principal::User(user_id));

        let first = handler
            .execute(&context, command(id))
            .await
            .unwrap_or_else(|error| panic!("first provider PUT failed: {error}"));
        let second = handler
            .execute(&context, command(id))
            .await
            .unwrap_or_else(|error| panic!("repeated provider PUT failed: {error}"));

        assert!(first.created);
        assert!(!second.created);
        let state = lock(&handler.sources.0);
        assert_eq!(2, state.locked_reads);
        assert_eq!(0, state.ordinary_reads);
        assert_eq!(1, state.updates);
        assert_eq!(2, state.commits);
    }

    #[test]
    fn normal_user_uses_open_world_capability_but_delegates_need_scope() {
        let user_id = UserId::new();
        assert!(matches!(
            actor(&context(Principal::User(user_id))),
            Ok(actor_id) if actor_id == user_id
        ));
        assert!(matches!(
            actor(&context(Principal::DelegatedUser {
                user_id,
                capabilities: std::collections::BTreeSet::from([
                    CredentialCapability::ListingSourcesWrite,
                ]),
            })),
            Ok(actor_id) if actor_id == user_id
        ));
        assert!(matches!(
            actor(&context(Principal::DelegatedUser {
                user_id,
                capabilities: Default::default(),
            })),
            Err(PutListingSourceIngestionConfigurationError::Forbidden)
        ));
    }
}
