use crate::ports::{
    ShopGeocoder, ShopGeocoderError, ShopRepository, ShopRepositoryError, ShopRepositoryFactory,
};
use common::currency::domain::Currency;
use common::domain::Domain;
use common::language::domain::Language;
use common::operation_context::OperationContext;
use common::transaction::{Transaction, UnitOfWork};
use common::write_metadata::WriteMetadata;
use common::{shop_id::ShopId, shop_name::ShopName, shop_slug_id::ShopSlugId};
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

#[derive(Debug, Clone, PartialEq)]
pub struct CreateShopResult {
    pub shop_id: ShopId,
    pub shop_slug_id: ShopSlugId,
    pub name: ShopName,
}

#[derive(Debug, thiserror::Error)]
pub enum CreateShopError {
    #[error("authenticated actor required to create shop")]
    AuthenticatedActorRequired,
    #[error("shop slug already exists")]
    SlugConflict,
    #[error("operation not permitted")]
    Forbidden,
    #[error("invalid shop address")]
    InvalidAddress,
    #[error("temporary shop persistence failure")]
    TemporarilyUnavailable,
    #[error("invalid persisted shop state")]
    InvalidPersistedState,
    #[error("internal shop persistence failure")]
    Internal,
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

pub struct CreateShopHandler<U, R, G> {
    unit_of_work: U,
    shops: R,
    geocoder: G,
}

impl<U, R, G> CreateShopHandler<U, R, G> {
    pub fn new(unit_of_work: U, shops: R, geocoder: G) -> Self {
        Self {
            unit_of_work,
            shops,
            geocoder,
        }
    }
}

#[async_trait::async_trait]
impl<U, R, G> CreateShopUseCase for CreateShopHandler<U, R, G>
where
    U: UnitOfWork,
    R: ShopRepositoryFactory<U::Tx>,
    G: ShopGeocoder,
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
        let metadata = WriteMetadata::try_from(context)
            .map_err(|_| CreateShopError::AuthenticatedActorRequired)?;
        tracing::Span::current().record("actor_id", tracing::field::display(metadata.actor()));

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
            return Err(CreateShopError::SlugConflict);
        }

        self.shops
            .in_transaction(&mut tx)
            .insert(&shop, &metadata)
            .await?;

        tx.commit()
            .await
            .map_err(|_| CreateShopError::CommitTransactionFailed)?;

        tracing::info!(
            event = "shop.created",
            actor_type = context.principal.kind(),
            actor_id = %metadata.actor(),
            shop_id = %shop.id(),
            shop_slug_id = %shop.slug_id(),
            outcome = "success",
        );

        Ok(CreateShopResult::from(&shop))
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

impl From<&Shop> for CreateShopResult {
    fn from(shop: &Shop) -> Self {
        Self {
            shop_id: shop.id(),
            shop_slug_id: shop.slug_id().clone(),
            name: shop.name().clone(),
        }
    }
}

impl From<ShopRepositoryError> for CreateShopError {
    fn from(error: ShopRepositoryError) -> Self {
        match error {
            ShopRepositoryError::SlugConflict => Self::SlugConflict,
            ShopRepositoryError::TemporarilyUnavailable => Self::TemporarilyUnavailable,
            ShopRepositoryError::InvalidPersistedState => Self::InvalidPersistedState,
            ShopRepositoryError::ConcurrencyConflict | ShopRepositoryError::Internal => {
                Self::Internal
            }
        }
    }
}

impl From<ShopGeocoderError> for CreateShopError {
    fn from(error: ShopGeocoderError) -> Self {
        match error {
            ShopGeocoderError::NotFound => Self::InvalidAddress,
            ShopGeocoderError::TemporarilyUnavailable => Self::TemporarilyUnavailable,
            ShopGeocoderError::Internal => Self::Internal,
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
    G: ShopGeocoder,
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
    use crate::ports::ShopStorageVersion;
    use common::operation_context::{CorrelationId, Principal, RequestId};
    use common::versioned::Versioned;
    use std::sync::{Arc, Mutex};

    #[derive(Default)]
    struct RepositoryState {
        existing_by_slug: Option<Versioned<Shop, ShopStorageVersion>>,
        inserted: Option<(Shop, WriteMetadata)>,
    }

    #[derive(Clone)]
    struct TestShopRepositoryFactory {
        state: Arc<Mutex<RepositoryState>>,
    }

    struct TestShopRepository {
        state: Arc<Mutex<RepositoryState>>,
    }

    struct TestUnitOfWork {
        committed: Arc<Mutex<bool>>,
    }

    struct TestTransaction {
        committed: Arc<Mutex<bool>>,
    }

    struct TestGeocoder;

    #[async_trait::async_trait]
    impl common::transaction::UnitOfWork for TestUnitOfWork {
        type Tx = TestTransaction;

        async fn begin(&self) -> Result<Self::Tx, common::transaction::TransactionError> {
            Ok(TestTransaction {
                committed: Arc::clone(&self.committed),
            })
        }
    }

    #[async_trait::async_trait]
    impl common::transaction::Transaction for TestTransaction {
        async fn commit(self) -> Result<(), common::transaction::TransactionError> {
            with_mutex(&self.committed, |committed| *committed = true);
            Ok(())
        }
    }

    impl crate::ports::ShopRepositoryFactory<TestTransaction> for TestShopRepositoryFactory {
        fn in_transaction<'tx>(
            &'tx self,
            _tx: &'tx mut TestTransaction,
        ) -> impl crate::ports::ShopRepository + 'tx {
            TestShopRepository {
                state: Arc::clone(&self.state),
            }
        }
    }

    #[async_trait::async_trait]
    impl crate::ports::ShopRepository for TestShopRepository {
        async fn find_by_id(
            &mut self,
            _id: ShopId,
        ) -> Result<Option<Versioned<Shop, ShopStorageVersion>>, ShopRepositoryError> {
            Ok(None)
        }

        async fn find_by_slug(
            &mut self,
            _slug_id: &ShopSlugId,
        ) -> Result<Option<Versioned<Shop, ShopStorageVersion>>, ShopRepositoryError> {
            Ok(with_mutex(&self.state, |state| {
                state.existing_by_slug.clone()
            }))
        }

        async fn insert(
            &mut self,
            shop: &Shop,
            metadata: &WriteMetadata,
        ) -> Result<(), ShopRepositoryError> {
            with_mutex(&self.state, |state| {
                state.inserted = Some((shop.clone(), metadata.clone()));
            });
            Ok(())
        }

        async fn update(
            &mut self,
            _shop: &Shop,
            _expected_version: ShopStorageVersion,
            _metadata: &WriteMetadata,
        ) -> Result<(), ShopRepositoryError> {
            Ok(())
        }
    }

    #[async_trait::async_trait]
    impl ShopGeocoder for TestGeocoder {
        async fn geocode(
            &self,
            _address: &StructuredAddress,
        ) -> Result<GeoAddress, ShopGeocoderError> {
            Ok(GeoAddress { lat: 1.0, lon: 2.0 })
        }
    }

    #[tokio::test]
    async fn should_create_shop_when_slug_free() {
        let state = Arc::new(Mutex::new(RepositoryState::default()));
        let committed = Arc::new(Mutex::new(false));
        let handler = CreateShopHandler::new(
            TestUnitOfWork {
                committed: Arc::clone(&committed),
            },
            TestShopRepositoryFactory {
                state: Arc::clone(&state),
            },
            TestGeocoder,
        );

        let result = handler.execute(&context(), command("Antik Markt")).await;

        assert!(matches!(result, Ok(ref value) if value.name == ShopName::from("Antik Markt")));
        assert!(with_mutex(&committed, |value| *value));
        let inserted = with_mutex(&state, |state| state.inserted.clone());
        assert!(matches!(inserted, Some((_, ref metadata)) if metadata.actor() == "SYSTEM"));
    }

    #[tokio::test]
    async fn should_reject_create_when_slug_exists() {
        let state = Arc::new(Mutex::new(RepositoryState::default()));
        with_mutex(&state, |state| {
            let shop = Shop::create(command("Antik Markt").into_new_shop(ShopId::new(), None));
            state.existing_by_slug = Some(Versioned::new(shop, ShopStorageVersion::INITIAL));
        });
        let committed = Arc::new(Mutex::new(false));
        let handler = CreateShopHandler::new(
            TestUnitOfWork {
                committed: Arc::clone(&committed),
            },
            TestShopRepositoryFactory {
                state: Arc::clone(&state),
            },
            TestGeocoder,
        );

        let result = handler.execute(&context(), command("Antik Markt")).await;

        assert!(matches!(result, Err(CreateShopError::SlugConflict)));
        assert!(!with_mutex(&committed, |value| *value));
    }

    fn context() -> OperationContext {
        OperationContext {
            principal: Principal::System,
            request_id: RequestId::from("request"),
            correlation_id: CorrelationId::from("correlation"),
        }
    }

    fn command(name: &str) -> CreateShopCommand {
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

    fn with_mutex<T, R>(mutex: &Mutex<T>, f: impl FnOnce(&mut T) -> R) -> R {
        match mutex.lock() {
            Ok(mut guard) => f(&mut guard),
            Err(poisoned) => {
                let mut guard = poisoned.into_inner();
                f(&mut guard)
            }
        }
    }
}
