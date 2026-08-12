use crate::ports::{
    PartnerShopApplicationRepository, PartnerShopApplicationRepositoryError,
    PartnerShopApplicationRepositoryFactory,
};
use common::currency::domain::Currency;
use common::domain::Domain;
use common::error::boxed::{BoxError, static_error};
use common::language::domain::Language;
use common::operation_context::{
    CredentialCapability, OperationAuthorizationError, OperationContext,
};
use common::transaction::{Transaction, UnitOfWork};
use common::{
    partner_shop_application_id::PartnerShopApplicationId, shop_id::ShopId, shop_name::ShopName,
};
use geo::{Geocoder, GeocodingError};
use serde_email::Email;
use shop_core::lifecycle::ShopLifecycle;
use shop_core::partner_status::ShopPartnerStatus;
use shop_core::shop::{
    NewShop, Shop, ShopAddress, ShopContact, ShopPresentation, ShopifyIntegration,
    WoocommerceIntegration,
};
use shop_core::shop_type::ShopType;
use shop_core::woocommerce_webhook_secret::WoocommerceWebhookSecret;
use shop_partner_core::partner_shop_application::{
    NewPartnerShopApplication, PartnerShopApplication, PartnerShopApplicationPayload,
};
use shop_service::ports::{ShopRepository, ShopRepositoryError, ShopRepositoryFactory};
use std::collections::HashSet;
use url::Url;

#[derive(Debug, Clone, PartialEq)]
pub struct CreatePartnerShopApplicationCommand {
    pub applicant_user_id: common::user_id::UserId,
    pub payload: CreatePartnerShopApplicationPayload,
}

#[derive(Debug, Clone, PartialEq)]
#[allow(clippy::large_enum_variant)]
pub enum CreatePartnerShopApplicationPayload {
    Existing { shop_id: ShopId },
    New(NewPartnerShopCommand),
}

#[derive(Debug, Clone, PartialEq)]
pub struct NewPartnerShopCommand {
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
    pub structured_address: Option<shop_core::address::StructuredAddress>,
    pub phone: Option<String>,
    pub email: Option<Email>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CreatePartnerShopApplicationResult {
    pub application: PartnerShopApplication,
}

#[allow(clippy::large_enum_variant)]
enum PreparedPartnerShopApplicationPayload {
    Existing { shop_id: ShopId },
    New(Shop),
}

#[derive(Debug, thiserror::Error)]
pub enum CreatePartnerShopApplicationError {
    #[error("authenticated actor required to create partner shop application")]
    AuthenticatedActorRequired,
    #[error("operation not permitted")]
    Forbidden,
    #[error("shop not found")]
    ShopNotFound,
    #[error("shop is not eligible for a partner application")]
    ShopNotEligible,
    #[error("shop slug already exists")]
    SlugConflict {
        #[source]
        source: BoxError,
    },
    #[error("invalid shop address")]
    InvalidAddress,
    #[error("temporary partner shop application persistence failure")]
    TemporarilyUnavailable {
        #[source]
        source: BoxError,
    },
    #[error("invalid persisted partner shop application state")]
    InvalidPersistedState {
        #[source]
        source: BoxError,
    },
    #[error("internal partner shop application persistence failure")]
    Internal {
        #[source]
        source: BoxError,
    },
    #[error("failed to begin create partner shop application transaction")]
    BeginTransactionFailed,
    #[error("failed to commit create partner shop application transaction")]
    CommitTransactionFailed,
}

#[async_trait::async_trait]
pub trait CreatePartnerShopApplicationUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        command: CreatePartnerShopApplicationCommand,
    ) -> Result<CreatePartnerShopApplicationResult, CreatePartnerShopApplicationError>;
}

pub struct CreatePartnerShopApplicationHandler<U, A, S, G> {
    unit_of_work: U,
    applications: A,
    shops: S,
    geocoder: G,
}

impl<U, A, S, G> CreatePartnerShopApplicationHandler<U, A, S, G> {
    pub fn new(unit_of_work: U, applications: A, shops: S, geocoder: G) -> Self {
        Self {
            unit_of_work,
            applications,
            shops,
            geocoder,
        }
    }
}

#[async_trait::async_trait]
impl<U, A, S, G> CreatePartnerShopApplicationUseCase
    for CreatePartnerShopApplicationHandler<U, A, S, G>
where
    U: UnitOfWork,
    A: PartnerShopApplicationRepositoryFactory<U::Tx>,
    S: ShopRepositoryFactory<U::Tx>,
    G: Geocoder,
{
    #[tracing::instrument(name = "create_partner_shop_application", skip_all, fields(applicant_user_id = %command.applicant_user_id, principal_type = context.principal.kind(), request_id = %context.request_id, correlation_id = %context.correlation_id))]
    async fn execute(
        &self,
        context: &OperationContext,
        command: CreatePartnerShopApplicationCommand,
    ) -> Result<CreatePartnerShopApplicationResult, CreatePartnerShopApplicationError> {
        authorize_create(context, command.applicant_user_id)?;

        let prepared_payload = match command.payload {
            CreatePartnerShopApplicationPayload::Existing { shop_id } => {
                PreparedPartnerShopApplicationPayload::Existing { shop_id }
            }
            CreatePartnerShopApplicationPayload::New(new_shop) => {
                PreparedPartnerShopApplicationPayload::New(
                    new_shop.into_draft_shop(&self.geocoder).await?,
                )
            }
        };

        let mut tx = self
            .unit_of_work
            .begin()
            .await
            .map_err(|_| CreatePartnerShopApplicationError::BeginTransactionFailed)?;

        let payload = match prepared_payload {
            PreparedPartnerShopApplicationPayload::Existing { shop_id } => {
                let shop = self
                    .shops
                    .in_transaction(&mut tx)
                    .find_by_id(shop_id)
                    .await?
                    .ok_or(CreatePartnerShopApplicationError::ShopNotFound)?;
                if shop.shop.lifecycle() == ShopLifecycle::Discarded {
                    return Err(CreatePartnerShopApplicationError::ShopNotEligible);
                }
                PartnerShopApplicationPayload::Existing { shop_id }
            }
            PreparedPartnerShopApplicationPayload::New(shop) => {
                if self
                    .shops
                    .in_transaction(&mut tx)
                    .find_by_slug(shop.slug_id())
                    .await?
                    .is_some()
                {
                    return Err(CreatePartnerShopApplicationError::SlugConflict {
                        source: static_error("shop slug already exists"),
                    });
                }
                let shop = self.shops.in_transaction(&mut tx).insert(&shop).await?.shop;
                PartnerShopApplicationPayload::New { shop_id: shop.id() }
            }
        };

        let application = PartnerShopApplication::create(NewPartnerShopApplication {
            id: PartnerShopApplicationId::new(),
            applicant_user_id: command.applicant_user_id,
            payload,
        });
        let application = self
            .applications
            .in_transaction(&mut tx)
            .insert(&application)
            .await?
            .value;

        tx.commit()
            .await
            .map_err(|_| CreatePartnerShopApplicationError::CommitTransactionFailed)?;

        Ok(CreatePartnerShopApplicationResult { application })
    }
}

impl NewPartnerShopCommand {
    async fn into_draft_shop<G>(
        self,
        geocoder: &G,
    ) -> Result<Shop, CreatePartnerShopApplicationError>
    where
        G: Geocoder,
    {
        let address = match self.structured_address {
            Some(structured) => Some(ShopAddress {
                geo: Some(geocoder.geocode(&structured).await?),
                structured,
            }),
            None => None,
        };
        Ok(Shop::create_with_lifecycle(
            NewShop {
                id: ShopId::new(),
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
                partner_status: ShopPartnerStatus::Scraped,
                affiliate_configuration: None,
            },
            ShopLifecycle::Drafted,
        ))
    }
}

fn woocommerce_integration(
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

impl From<OperationAuthorizationError> for CreatePartnerShopApplicationError {
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

impl From<ShopRepositoryError> for CreatePartnerShopApplicationError {
    fn from(error: ShopRepositoryError) -> Self {
        match error {
            ShopRepositoryError::ConcurrencyConflict => Self::Internal {
                source: static_error("unexpected shop concurrency conflict"),
            },
            ShopRepositoryError::SlugConflict { source } => Self::SlugConflict { source },
            ShopRepositoryError::TemporarilyUnavailable { source } => {
                Self::TemporarilyUnavailable { source }
            }
            ShopRepositoryError::InvalidPersistedState { source } => {
                Self::InvalidPersistedState { source }
            }
            ShopRepositoryError::Internal { source } => Self::Internal { source },
        }
    }
}

fn authorize_create(
    context: &OperationContext,
    applicant_user_id: common::user_id::UserId,
) -> Result<(), CreatePartnerShopApplicationError> {
    context
        .require()
        .credential_capability(CredentialCapability::PartnerShopApplicationsWrite)
        .user(&applicant_user_id)
        .service_or_system()
        .authorize::<CreatePartnerShopApplicationError>()
}

impl From<PartnerShopApplicationRepositoryError> for CreatePartnerShopApplicationError {
    fn from(error: PartnerShopApplicationRepositoryError) -> Self {
        match error {
            PartnerShopApplicationRepositoryError::ConcurrencyConflict => Self::Internal {
                source: static_error("unexpected partner application concurrency conflict"),
            },
            PartnerShopApplicationRepositoryError::TemporarilyUnavailable { source } => {
                Self::TemporarilyUnavailable { source }
            }
            PartnerShopApplicationRepositoryError::InvalidPersistedState { source } => {
                Self::InvalidPersistedState { source }
            }
            PartnerShopApplicationRepositoryError::Internal { source } => Self::Internal { source },
        }
    }
}

impl From<GeocodingError> for CreatePartnerShopApplicationError {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, thiserror::Error)]
    #[error("simulated temporary geocoding failure")]
    struct TemporaryGeocodingFailure;

    #[derive(Debug, thiserror::Error)]
    #[error("simulated internal geocoding failure")]
    struct InternalGeocodingFailure;

    #[test]
    fn should_map_geocoding_errors_preserving_sources() {
        assert!(matches!(
            CreatePartnerShopApplicationError::from(GeocodingError::NotFound),
            CreatePartnerShopApplicationError::InvalidAddress
        ));

        let temporary = CreatePartnerShopApplicationError::from(
            GeocodingError::temporarily_unavailable(TemporaryGeocodingFailure),
        );
        assert!(matches!(
            temporary,
            CreatePartnerShopApplicationError::TemporarilyUnavailable { ref source }
                if source.to_string() == "simulated temporary geocoding failure"
        ));

        let internal = CreatePartnerShopApplicationError::from(GeocodingError::internal(
            InternalGeocodingFailure,
        ));
        assert!(matches!(
            internal,
            CreatePartnerShopApplicationError::Internal { ref source }
                if source.to_string() == "simulated internal geocoding failure"
        ));
    }
}
