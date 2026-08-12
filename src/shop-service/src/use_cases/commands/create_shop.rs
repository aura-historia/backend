use crate::ports::{ShopRepository, ShopRepositoryError, ShopRepositoryFactory};
use crate::use_cases::queries::get_shop::ShopDetailsView;
use common::currency::domain::Currency;
use common::domain::Domain;
use common::error::boxed::{BoxError, static_error};
use common::language::domain::Language;
use common::operation_context::{
    CredentialCapability, OperationAuthorizationError, OperationContext, Principal,
};
use common::transaction::{Transaction, UnitOfWork};
use common::{shop_id::ShopId, shop_name::ShopName, shop_slug_id::ShopSlugId, user_id::UserId};
use geo::{Geocoder, GeocodingError};
use serde_email::Email;
use shop_core::{
    address::{GeoAddress, StructuredAddress},
    affiliate_configuration::AffiliateConfiguration,
    partner_status::ShopPartnerStatus,
    shop::{
        NewShop, Shop, ShopAddress, ShopContact, ShopPresentation, ShopifyIntegration,
        WoocommerceIntegration,
    },
    shop_type::ShopType,
    woocommerce_webhook_secret::WoocommerceWebhookSecret,
};
use std::collections::HashSet;
use url::Url;
use user_service::use_cases::queries::check_user_admin::{
    CheckUserAdminError, CheckUserAdminRequest, CheckUserAdminUseCase,
};

#[derive(Debug, Clone, PartialEq)]
pub struct CreateShopCommand {
    pub name: ShopName,
    pub shop_type: ShopType,
    pub domains: HashSet<Domain>,
    pub shopify_domain: Option<Domain>,
    pub shopify_currency: Option<Currency>,
    pub shopify_language: Option<Language>,
    pub woocommerce_webhook_secret: Option<WoocommerceWebhookSecret>,
    pub woocommerce_currency: Option<Currency>,
    pub woocommerce_language: Option<Language>,
    pub url: Option<Url>,
    pub image: Option<Url>,
    pub structured_address: Option<StructuredAddress>,
    pub phone: Option<String>,
    pub email: Option<Email>,
    pub affiliate_configuration: Option<AffiliateConfiguration>,
}

pub type CreateShopResult = ShopDetailsView;

#[derive(Debug, thiserror::Error)]
pub enum CreateShopError {
    #[error("authenticated actor required to create shop")]
    AuthenticatedActorRequired,
    #[error("shop slug already exists")]
    SlugConflict {
        #[source]
        source: BoxError,
    },
    #[error("operation not permitted")]
    Forbidden,
    #[error("invalid shop address")]
    InvalidAddress,
    #[error("temporary shop persistence failure")]
    TemporarilyUnavailable {
        #[source]
        source: BoxError,
    },
    #[error("invalid persisted shop state")]
    InvalidPersistedState {
        #[source]
        source: BoxError,
    },
    #[error("internal shop persistence failure")]
    Internal {
        #[source]
        source: BoxError,
    },
    #[error("failed to begin create shop transaction")]
    BeginTransactionFailed,
    #[error("failed to commit create shop transaction")]
    CommitTransactionFailed,
}

#[async_trait::async_trait]
pub trait CreateShopUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        command: CreateShopCommand,
    ) -> Result<CreateShopResult, CreateShopError>;
}

pub struct CreateShopHandler<U, R, G, A> {
    unit_of_work: U,
    shops: R,
    geocoder: G,
    check_user_admin: A,
}

impl<U, R, G, A> CreateShopHandler<U, R, G, A> {
    pub fn new(unit_of_work: U, shops: R, geocoder: G, check_user_admin: A) -> Self {
        Self {
            unit_of_work,
            shops,
            geocoder,
            check_user_admin,
        }
    }
}

#[async_trait::async_trait]
impl<U, R, G, A> CreateShopUseCase for CreateShopHandler<U, R, G, A>
where
    U: UnitOfWork,
    R: ShopRepositoryFactory<U::Tx>,
    G: Geocoder,
    A: CheckUserAdminUseCase,
{
    #[tracing::instrument(
        name = "create_shop",
        skip_all,
        fields(
            shop_name = %command.name,
            principal_type = context.principal.kind(),
            actor_id = tracing::field::Empty,
            request_id = %context.request_id,
            correlation_id = %context.correlation_id,
        )
    )]
    async fn execute(
        &self,
        context: &OperationContext,
        command: CreateShopCommand,
    ) -> Result<CreateShopResult, CreateShopError> {
        context
            .require()
            .credential_capability(CredentialCapability::ShopsWrite)
            .authorize::<CreateShopError>()?;
        ensure_can_create_shop(context, &self.check_user_admin).await?;
        tracing::Span::current().record(
            "actor_id",
            tracing::field::display(context.principal.label()),
        );

        let slug_id = ShopSlugId::from(command.name.as_ref());
        let address = geocode_address(command.structured_address.clone(), &self.geocoder).await?;
        let shop = Shop::create(command.into_new_shop(ShopId::new(), address));

        let mut tx = self
            .unit_of_work
            .begin()
            .await
            .map_err(|_| CreateShopError::BeginTransactionFailed)?;

        if self
            .shops
            .in_transaction(&mut tx)
            .find_by_slug(&slug_id)
            .await?
            .is_some()
        {
            return Err(CreateShopError::SlugConflict {
                source: static_error("shop slug already exists"),
            });
        }

        let view = self
            .shops
            .in_transaction(&mut tx)
            .insert(&shop)
            .await?
            .into();

        tx.commit()
            .await
            .map_err(|_| CreateShopError::CommitTransactionFailed)?;

        tracing::info!(
            event = "shop.created",
            actor_type = context.principal.kind(),
            actor_id = %context.principal.label(),
            shop_id = %shop.id(),
            shop_slug_id = %shop.slug_id(),
            outcome = "success",
        );

        Ok(view)
    }
}

impl CreateShopCommand {
    pub fn into_new_shop(self, shop_id: ShopId, address: Option<ShopAddress>) -> NewShop {
        NewShop {
            id: shop_id,
            name: self.name,
            shop_type: self.shop_type,
            domains: self.domains,
            shopify: self.shopify_domain.map(|domain| ShopifyIntegration {
                domain,
                currency: self.shopify_currency,
                language: self.shopify_language,
            }),
            woocommerce: woocommerce_integration(
                self.woocommerce_webhook_secret,
                self.woocommerce_currency,
                self.woocommerce_language,
            ),
            presentation: ShopPresentation {
                url: self.url,
                image: self.image,
            },
            address,
            contact: ShopContact {
                phone: self.phone,
                email: self.email,
            },
            partner_status: ShopPartnerStatus::default(),
            affiliate_configuration: self.affiliate_configuration,
        }
    }
}

impl From<OperationAuthorizationError> for CreateShopError {
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

impl From<ShopRepositoryError> for CreateShopError {
    fn from(error: ShopRepositoryError) -> Self {
        match error {
            ShopRepositoryError::SlugConflict { source } => Self::SlugConflict { source },
            ShopRepositoryError::TemporarilyUnavailable { source } => {
                Self::TemporarilyUnavailable { source }
            }
            ShopRepositoryError::InvalidPersistedState { source } => {
                Self::InvalidPersistedState { source }
            }
            ShopRepositoryError::ConcurrencyConflict => Self::Internal {
                source: static_error("unexpected create shop concurrency conflict"),
            },
            ShopRepositoryError::Internal { source } => Self::Internal { source },
        }
    }
}

async fn ensure_can_create_shop<A>(
    context: &OperationContext,
    check_user_admin: &A,
) -> Result<(), CreateShopError>
where
    A: CheckUserAdminUseCase,
{
    if actor_user_id(context)?.is_none() {
        return Ok(());
    }

    check_user_admin
        .execute(context, CheckUserAdminRequest)
        .await
        .map(drop)
        .map_err(map_admin_error)
}

fn actor_user_id(context: &OperationContext) -> Result<Option<UserId>, CreateShopError> {
    match context.principal {
        Principal::Anonymous => Err(CreateShopError::AuthenticatedActorRequired),
        Principal::User(user_id) | Principal::DelegatedUser { user_id, .. } => Ok(Some(user_id)),
        Principal::Service(_) | Principal::System => Ok(None),
    }
}

fn map_admin_error(error: CheckUserAdminError) -> CreateShopError {
    match error {
        CheckUserAdminError::AuthenticatedActorRequired => {
            CreateShopError::AuthenticatedActorRequired
        }
        CheckUserAdminError::Forbidden => CreateShopError::Forbidden,
        CheckUserAdminError::TemporarilyUnavailable { source } => {
            CreateShopError::TemporarilyUnavailable { source }
        }
        CheckUserAdminError::InvalidReadModel { source }
        | CheckUserAdminError::Internal { source } => CreateShopError::Internal { source },
        CheckUserAdminError::BeginTransactionFailed
        | CheckUserAdminError::CommitTransactionFailed => CreateShopError::TemporarilyUnavailable {
            source: static_error("check user admin transaction failed"),
        },
    }
}

impl From<GeocodingError> for CreateShopError {
    fn from(error: GeocodingError) -> Self {
        match error {
            GeocodingError::NotFound => Self::InvalidAddress,
            GeocodingError::TemporarilyUnavailable { source } => {
                Self::TemporarilyUnavailable { source }
            }
            GeocodingError::Internal { source } => Self::Internal { source },
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct GeocodedShopAddress {
    pub structured_address: Option<StructuredAddress>,
    pub geo_address: Option<GeoAddress>,
}

async fn geocode_address<G>(
    structured_address: Option<StructuredAddress>,
    geocoder: &G,
) -> Result<Option<ShopAddress>, CreateShopError>
where
    G: Geocoder,
{
    match structured_address {
        Some(structured) => {
            let geo = geocoder.geocode(&structured).await?;
            Ok(Some(ShopAddress {
                structured,
                geo: Some(geo),
            }))
        }
        None => Ok(None),
    }
}

pub(crate) fn woocommerce_integration(
    webhook_secret: Option<WoocommerceWebhookSecret>,
    currency: Option<Currency>,
    language: Option<Language>,
) -> Option<WoocommerceIntegration> {
    if webhook_secret.is_none() && currency.is_none() && language.is_none() {
        None
    } else {
        Some(WoocommerceIntegration {
            webhook_secret,
            currency,
            language,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ports::{
        ShopRepository, ShopRepositoryError, ShopRepositoryFactory, ShopStorageVersion, StoredShop,
    };
    use common::error::boxed::static_error;
    use common::operation_context::{CorrelationId, Principal, RequestId};
    use common::transaction::{TransactionError, UnitOfWork};
    use shop_core::shop::{NewShop, ShopContact, ShopPresentation};
    use std::sync::{Arc, Mutex};
    use user_service::use_cases::queries::check_user_admin::{
        CheckUserAdminRequest, CheckUserAdminResult,
    };

    #[derive(Clone, Copy)]
    enum RepoErrorKind {
        TemporarilyUnavailable,
        InvalidPersistedState,
    }

    #[derive(Clone, Copy)]
    enum GeocodingErrorKind {
        NotFound,
        TemporarilyUnavailable,
    }

    #[derive(Debug, thiserror::Error)]
    #[error("simulated temporary geocoding failure")]
    struct TemporaryGeocodingFailure;

    #[derive(Debug, thiserror::Error)]
    #[error("simulated internal geocoding failure")]
    struct InternalGeocodingFailure;

    #[derive(Default)]
    struct Counts {
        begin: usize,
        commit: usize,
        find_by_slug: usize,
        insert: usize,
        geocode: usize,
    }

    #[derive(Default)]
    struct State {
        begin_error: bool,
        commit_error: bool,
        shop_by_slug: Option<StoredShop>,
        find_by_slug_error: Option<RepoErrorKind>,
        insert_error: Option<RepoErrorKind>,
        geocoder_error: Option<GeocodingErrorKind>,
        inserted: Option<Shop>,
        counts: Counts,
    }

    #[derive(Clone, Default)]
    struct FakeUnitOfWork {
        state: Arc<Mutex<State>>,
    }

    #[derive(Clone, Default)]
    struct FakeShopRepositoryFactory {
        state: Arc<Mutex<State>>,
    }

    #[derive(Clone, Default)]
    struct FakeGeocoder {
        state: Arc<Mutex<State>>,
    }

    struct FakeTx {
        state: Arc<Mutex<State>>,
    }

    struct FakeShopRepository {
        state: Arc<Mutex<State>>,
    }

    #[derive(Clone, Copy)]
    struct AllowPolicy;

    #[async_trait::async_trait]
    impl UnitOfWork for FakeUnitOfWork {
        type Tx = FakeTx;

        async fn begin(&self) -> Result<Self::Tx, TransactionError> {
            let fail = with_state(&self.state, |state| {
                state.counts.begin += 1;
                state.begin_error
            });
            if fail {
                Err(TransactionError::BeginFailed)
            } else {
                Ok(FakeTx {
                    state: Arc::clone(&self.state),
                })
            }
        }
    }

    #[async_trait::async_trait]
    impl Transaction for FakeTx {
        async fn commit(self) -> Result<(), TransactionError> {
            let fail = with_state(&self.state, |state| {
                state.counts.commit += 1;
                state.commit_error
            });
            if fail {
                Err(TransactionError::CommitFailed)
            } else {
                Ok(())
            }
        }
    }

    impl ShopRepositoryFactory<FakeTx> for FakeShopRepositoryFactory {
        fn in_transaction<'tx>(&'tx self, _tx: &'tx mut FakeTx) -> impl ShopRepository + 'tx {
            FakeShopRepository {
                state: Arc::clone(&self.state),
            }
        }
    }

    #[async_trait::async_trait]
    impl ShopRepository for FakeShopRepository {
        async fn find_by_id(
            &mut self,
            _id: ShopId,
        ) -> Result<Option<StoredShop>, ShopRepositoryError> {
            Ok(None)
        }

        async fn find_by_slug(
            &mut self,
            _slug_id: &ShopSlugId,
        ) -> Result<Option<StoredShop>, ShopRepositoryError> {
            with_state(&self.state, |state| {
                state.counts.find_by_slug += 1;
                match state.find_by_slug_error {
                    Some(kind) => Err(shop_repo_error(kind)),
                    None => Ok(state.shop_by_slug.clone()),
                }
            })
        }

        async fn insert(&mut self, shop: &Shop) -> Result<StoredShop, ShopRepositoryError> {
            with_state(&self.state, |state| {
                state.counts.insert += 1;
                match state.insert_error {
                    Some(kind) => Err(shop_repo_error(kind)),
                    None => {
                        state.inserted = Some(shop.clone());
                        Ok(stored_shop(shop.clone()))
                    }
                }
            })
        }

        async fn update(
            &mut self,
            _shop: &Shop,
            _expected_version: ShopStorageVersion,
        ) -> Result<StoredShop, ShopRepositoryError> {
            Ok(stored_shop(_shop.clone()))
        }
    }

    #[async_trait::async_trait]
    impl CheckUserAdminUseCase for AllowPolicy {
        async fn execute(
            &self,
            _context: &OperationContext,
            request: CheckUserAdminRequest,
        ) -> Result<CheckUserAdminResult, CheckUserAdminError> {
            let _ = request;
            Ok(CheckUserAdminResult)
        }
    }

    #[async_trait::async_trait]
    impl Geocoder for FakeGeocoder {
        async fn geocode(
            &self,
            _address: &StructuredAddress,
        ) -> Result<GeoAddress, GeocodingError> {
            with_state(&self.state, |state| {
                state.counts.geocode += 1;
                match state.geocoder_error {
                    Some(GeocodingErrorKind::NotFound) => Err(GeocodingError::NotFound),
                    Some(GeocodingErrorKind::TemporarilyUnavailable) => Err(
                        GeocodingError::temporarily_unavailable(TemporaryGeocodingFailure),
                    ),
                    None => Ok(GeoAddress { lat: 1.0, lon: 2.0 }),
                }
            })
        }
    }

    #[test]
    fn should_map_geocoding_errors_preserving_sources() {
        assert!(matches!(
            CreateShopError::from(GeocodingError::NotFound),
            CreateShopError::InvalidAddress
        ));

        let temporary = CreateShopError::from(GeocodingError::temporarily_unavailable(
            TemporaryGeocodingFailure,
        ));
        assert!(matches!(
            temporary,
            CreateShopError::TemporarilyUnavailable { ref source }
                if source.to_string() == "simulated temporary geocoding failure"
        ));

        let internal = CreateShopError::from(GeocodingError::internal(InternalGeocodingFailure));
        assert!(matches!(
            internal,
            CreateShopError::Internal { ref source }
                if source.to_string() == "simulated internal geocoding failure"
        ));
    }

    #[tokio::test]
    async fn should_create_shop_when_slug_free() {
        let state = shared_state();
        let handler = build_handler(&state);

        let result = handler
            .execute(&system_context(), create_command("Antik Markt"))
            .await;

        assert!(matches!(result, Ok(ref value) if value.name == ShopName::from("Antik Markt")));
        assert_counts(&state, |counts| {
            assert_eq!(1, counts.begin);
            assert_eq!(1, counts.find_by_slug);
            assert_eq!(1, counts.insert);
            assert_eq!(1, counts.commit);
        });
        assert!(with_state(&state, |state| state.inserted.is_some()));
    }

    #[tokio::test]
    async fn should_not_begin_create_when_geocoder_fails() {
        let state = shared_state();
        with_state(&state, |state| {
            state.geocoder_error = Some(GeocodingErrorKind::NotFound)
        });
        let handler = build_handler(&state);

        let result = handler
            .execute(
                &system_context(),
                create_command_with_address("Antik Markt"),
            )
            .await;

        assert!(matches!(result, Err(CreateShopError::InvalidAddress)));
        assert_counts(&state, |counts| {
            assert_eq!(1, counts.geocode);
            assert_eq!(0, counts.begin);
            assert_eq!(0, counts.commit);
        });
    }

    #[tokio::test]
    async fn should_map_create_begin_and_commit_failures() {
        let state = shared_state();
        with_state(&state, |state| state.begin_error = true);
        let begin_handler = build_handler(&state);

        let begin_result = begin_handler
            .execute(&system_context(), create_command("Begin Fail"))
            .await;

        assert!(matches!(
            begin_result,
            Err(CreateShopError::BeginTransactionFailed)
        ));

        let state = shared_state();
        with_state(&state, |state| state.commit_error = true);
        let commit_handler = build_handler(&state);

        let commit_result = commit_handler
            .execute(&system_context(), create_command("Commit Fail"))
            .await;

        assert!(matches!(
            commit_result,
            Err(CreateShopError::CommitTransactionFailed)
        ));
        assert_counts(&state, |counts| assert_eq!(1, counts.commit));
    }

    #[tokio::test]
    async fn should_not_commit_create_when_slug_exists_or_repo_fails() {
        let state = shared_state();
        with_state(&state, |state| {
            state.shop_by_slug = Some(stored_shop(shop("Antik Markt")))
        });
        let slug_handler = build_handler(&state);

        let slug_result = slug_handler
            .execute(&system_context(), create_command("Antik Markt"))
            .await;

        assert!(matches!(
            slug_result,
            Err(CreateShopError::SlugConflict { .. })
        ));
        assert_counts(&state, |counts| {
            assert_eq!(0, counts.insert);
            assert_eq!(0, counts.commit);
        });

        let state = shared_state();
        with_state(&state, |state| {
            state.insert_error = Some(RepoErrorKind::TemporarilyUnavailable)
        });
        let insert_handler = build_handler(&state);

        let insert_result = insert_handler
            .execute(&system_context(), create_command("Repo Fail"))
            .await;

        assert!(matches!(
            insert_result,
            Err(CreateShopError::TemporarilyUnavailable { .. })
        ));
        assert_counts(&state, |counts| assert_eq!(0, counts.commit));
    }

    #[tokio::test]
    async fn should_map_create_repo_and_geocoder_errors() {
        let state = shared_state();
        with_state(&state, |state| {
            state.find_by_slug_error = Some(RepoErrorKind::InvalidPersistedState)
        });
        let handler = build_handler(&state);

        let repo_result = handler
            .execute(&system_context(), create_command("Bad Read"))
            .await;

        assert!(matches!(
            repo_result,
            Err(CreateShopError::InvalidPersistedState { .. })
        ));

        let state = shared_state();
        with_state(&state, |state| {
            state.geocoder_error = Some(GeocodingErrorKind::TemporarilyUnavailable)
        });
        let handler = build_handler(&state);

        let geo_result = handler
            .execute(&system_context(), create_command_with_address("Bad Geo"))
            .await;

        assert!(matches!(
            geo_result,
            Err(CreateShopError::TemporarilyUnavailable { ref source })
                if source.to_string() == "simulated temporary geocoding failure"
        ));
    }

    fn build_handler(state: &Arc<Mutex<State>>) -> impl CreateShopUseCase {
        CreateShopHandler::new(uow(state), shop_repo(state), geocoder(state), AllowPolicy)
    }

    fn shop_repo(state: &Arc<Mutex<State>>) -> FakeShopRepositoryFactory {
        FakeShopRepositoryFactory {
            state: Arc::clone(state),
        }
    }

    fn geocoder(state: &Arc<Mutex<State>>) -> FakeGeocoder {
        FakeGeocoder {
            state: Arc::clone(state),
        }
    }

    fn uow(state: &Arc<Mutex<State>>) -> FakeUnitOfWork {
        FakeUnitOfWork {
            state: Arc::clone(state),
        }
    }

    fn shared_state() -> Arc<Mutex<State>> {
        Arc::new(Mutex::new(State::default()))
    }

    fn shop_repo_error(kind: RepoErrorKind) -> ShopRepositoryError {
        match kind {
            RepoErrorKind::TemporarilyUnavailable => ShopRepositoryError::TemporarilyUnavailable {
                source: static_error("temporary"),
            },
            RepoErrorKind::InvalidPersistedState => ShopRepositoryError::InvalidPersistedState {
                source: static_error("invalid"),
            },
        }
    }

    fn create_command(name: &str) -> CreateShopCommand {
        CreateShopCommand {
            name: ShopName::from(name),
            shop_type: ShopType::CommercialDealer,
            domains: HashSet::new(),
            shopify_domain: None,
            shopify_currency: None,
            shopify_language: None,
            woocommerce_webhook_secret: None,
            woocommerce_currency: None,
            woocommerce_language: None,
            url: None,
            image: None,
            structured_address: None,
            phone: None,
            email: None,
            affiliate_configuration: None,
        }
    }

    fn create_command_with_address(name: &str) -> CreateShopCommand {
        CreateShopCommand {
            structured_address: Some(address()),
            ..create_command(name)
        }
    }

    fn shop(name: &str) -> Shop {
        Shop::create(NewShop {
            id: ShopId::new(),
            name: ShopName::from(name),
            shop_type: ShopType::CommercialDealer,
            domains: HashSet::new(),
            shopify: None,
            woocommerce: None,
            presentation: ShopPresentation::default(),
            address: None,
            contact: ShopContact::default(),
            partner_status: ShopPartnerStatus::Scraped,
            affiliate_configuration: None,
        })
    }

    fn stored_shop(shop: Shop) -> StoredShop {
        StoredShop {
            shop,
            version: ShopStorageVersion::INITIAL,
            created: time::OffsetDateTime::now_utc(),
            updated: time::OffsetDateTime::now_utc(),
        }
    }
    fn address() -> StructuredAddress {
        StructuredAddress {
            addressline: Some("Street 1".to_string()),
            addressline_extra: None,
            locality: Some("Berlin".to_string()),
            region: None,
            postal_code: Some("10115".to_string()),
            country: None,
            continent: None,
        }
    }

    fn system_context() -> OperationContext {
        OperationContext {
            principal: Principal::System,
            request_id: RequestId::from("request"),
            correlation_id: CorrelationId::from("correlation"),
        }
    }

    fn assert_counts(state: &Arc<Mutex<State>>, assert: impl FnOnce(&Counts)) {
        with_state(state, |state| assert(&state.counts));
    }

    fn with_state<R>(state: &Arc<Mutex<State>>, f: impl FnOnce(&mut State) -> R) -> R {
        match state.lock() {
            Ok(mut guard) => f(&mut guard),
            Err(poisoned) => {
                let mut guard = poisoned.into_inner();
                f(&mut guard)
            }
        }
    }
}
