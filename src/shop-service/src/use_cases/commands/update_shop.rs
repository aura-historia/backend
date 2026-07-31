use crate::ports::{
    ShopDetailsReadError, ShopDetailsReader, ShopDetailsReaderFactory, ShopGeocoder,
    ShopGeocoderError, ShopRepository, ShopRepositoryError, ShopRepositoryFactory, ShopWritePolicy,
    ShopWritePolicyError,
};
use crate::use_cases::commands::create_shop::woocommerce_integration;
use crate::use_cases::queries::get_shop::{GetShopRequest, ShopDetailsView};
use common::change_outcome::ChangeOutcome;
use common::currency::domain::Currency;
use common::domain::Domain;
use common::error::boxed::{BoxError, static_error};
use common::language::domain::Language;
use common::operation_context::{
    CredentialCapability, OperationAuthorizationError, OperationContext,
};
use common::patch_field::PatchField;
use common::shop_id::ShopId;
use common::transaction::{Transaction, UnitOfWork};
use serde_email::Email;
use shop_core::{
    address::StructuredAddress,
    affiliate_configuration::AffiliateConfiguration,
    shop::{
        Shop, ShopAddress, ShopContact, ShopPresentation, ShopifyIntegration,
        WoocommerceIntegration,
    },
    shop_type::ShopType,
    woocommerce_webhook_secret::WoocommerceWebhookSecret,
};
use std::collections::HashSet;
use url::Url;

#[derive(Debug, Clone, PartialEq, Default)]
pub struct UpdateShopCommand {
    pub shop_id: ShopId,
    pub shop_type: PatchField<ShopType>,
    pub domains: PatchField<HashSet<Domain>>,
    pub shopify_domain: PatchField<Domain>,
    pub shopify_currency: PatchField<Currency>,
    pub shopify_language: PatchField<Language>,
    pub woocommerce_webhook_secret: PatchField<WoocommerceWebhookSecret>,
    pub woocommerce_currency: PatchField<Currency>,
    pub woocommerce_language: PatchField<Language>,
    pub url: PatchField<Url>,
    pub image: PatchField<Url>,
    pub structured_address: PatchField<StructuredAddress>,
    pub phone: PatchField<String>,
    pub email: PatchField<Email>,
    pub affiliate_configuration: PatchField<AffiliateConfiguration>,
}

impl UpdateShopCommand {
    pub fn is_empty(&self) -> bool {
        !self.shop_type.is_changed()
            && !self.domains.is_changed()
            && !self.shopify_domain.is_changed()
            && !self.shopify_currency.is_changed()
            && !self.shopify_language.is_changed()
            && !self.woocommerce_webhook_secret.is_changed()
            && !self.woocommerce_currency.is_changed()
            && !self.woocommerce_language.is_changed()
            && !self.url.is_changed()
            && !self.image.is_changed()
            && !self.structured_address.is_changed()
            && !self.phone.is_changed()
            && !self.email.is_changed()
            && !self.affiliate_configuration.is_changed()
    }
}

pub type UpdateShopResult = ShopDetailsView;

#[derive(Debug, thiserror::Error)]
pub enum UpdateShopError {
    #[error("authenticated actor required to update shop")]
    AuthenticatedActorRequired,
    #[error("operation not permitted")]
    Forbidden,
    #[error("shop not found")]
    ShopNotFound,
    #[error("concurrent shop update")]
    ConcurrencyConflict,
    #[error("shop slug already exists")]
    SlugConflict {
        #[source]
        source: BoxError,
    },
    #[error("shop type is required")]
    ShopTypeRequired,
    #[error("shop domains are required")]
    DomainsRequired,
    #[error("shopify domain is required when changing shopify settings")]
    ShopifyDomainRequired,
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
    #[error("failed to begin update shop transaction")]
    BeginTransactionFailed,
    #[error("failed to commit update shop transaction")]
    CommitTransactionFailed,
}

#[async_trait::async_trait]
pub trait UpdateShopUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        command: UpdateShopCommand,
    ) -> Result<UpdateShopResult, UpdateShopError>;
}

pub struct UpdateShopHandler<U, R, D, G, P> {
    unit_of_work: U,
    shops: R,
    details: D,
    geocoder: G,
    policy: P,
}

impl<U, R, D, G, P> UpdateShopHandler<U, R, D, G, P> {
    pub fn new(unit_of_work: U, shops: R, details: D, geocoder: G, policy: P) -> Self {
        Self {
            unit_of_work,
            shops,
            details,
            geocoder,
            policy,
        }
    }
}

#[async_trait::async_trait]
impl<U, R, D, G, P> UpdateShopUseCase for UpdateShopHandler<U, R, D, G, P>
where
    U: UnitOfWork,
    R: ShopRepositoryFactory<U::Tx>,
    D: ShopDetailsReaderFactory<U::Tx>,
    G: ShopGeocoder,
    P: ShopWritePolicy,
{
    #[tracing::instrument(
        name = "update_shop",
        skip_all,
        fields(
            shop_id = %command.shop_id,
            principal_type = context.principal.kind(),
            actor_id = tracing::field::Empty,
            request_id = %context.request_id,
            correlation_id = %context.correlation_id,
        )
    )]
    async fn execute(
        &self,
        context: &OperationContext,
        command: UpdateShopCommand,
    ) -> Result<UpdateShopResult, UpdateShopError> {
        context
            .require()
            .credential_capability(CredentialCapability::ShopsWrite)
            .authorize::<UpdateShopError>()?;
        self.policy
            .ensure_can_update_shop(context, command.shop_id)
            .await?;
        tracing::Span::current().record(
            "actor_id",
            tracing::field::display(context.principal.label()),
        );

        let command = prepare_update(command, &self.geocoder).await?;

        let mut tx = self
            .unit_of_work
            .begin()
            .await
            .map_err(|_| UpdateShopError::BeginTransactionFailed)?;

        let common::versioned::Versioned {
            value: mut shop,
            version,
        } = self
            .shops
            .in_transaction(&mut tx)
            .find_by_id(command.shop_id)
            .await?
            .ok_or(UpdateShopError::ShopNotFound)?;

        let outcome = apply_update(&mut shop, command)?;

        if outcome.changed() {
            self.shops
                .in_transaction(&mut tx)
                .update(&shop, version)
                .await?;
        }

        let view = self
            .details
            .in_transaction(&mut tx)
            .find_details(&GetShopRequest::ById(shop.id()))
            .await?
            .ok_or(UpdateShopError::InvalidPersistedState {
                source: static_error("updated shop details not found"),
            })?;

        tx.commit()
            .await
            .map_err(|_| UpdateShopError::CommitTransactionFailed)?;

        tracing::info!(
            event = "shop.updated",
            actor_type = context.principal.kind(),
            actor_id = %context.principal.label(),
            shop_id = %shop.id(),
            shop_slug_id = %shop.slug_id(),
            changed = outcome.changed(),
            outcome = "success",
        );

        Ok(view)
    }
}

impl From<OperationAuthorizationError> for UpdateShopError {
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

impl From<ShopRepositoryError> for UpdateShopError {
    fn from(error: ShopRepositoryError) -> Self {
        match error {
            ShopRepositoryError::ConcurrencyConflict => Self::ConcurrencyConflict,
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

impl From<ShopWritePolicyError> for UpdateShopError {
    fn from(error: ShopWritePolicyError) -> Self {
        match error {
            ShopWritePolicyError::Forbidden => Self::Forbidden,
            ShopWritePolicyError::TemporarilyUnavailable { source } => {
                Self::TemporarilyUnavailable { source }
            }
            ShopWritePolicyError::Internal { source } => Self::Internal { source },
        }
    }
}

impl From<ShopDetailsReadError> for UpdateShopError {
    fn from(error: ShopDetailsReadError) -> Self {
        match error {
            ShopDetailsReadError::TemporarilyUnavailable { source } => {
                Self::TemporarilyUnavailable { source }
            }
            ShopDetailsReadError::InvalidReadModel { source } => {
                Self::InvalidPersistedState { source }
            }
            ShopDetailsReadError::Internal { source } => Self::Internal { source },
        }
    }
}

impl From<ShopGeocoderError> for UpdateShopError {
    fn from(error: ShopGeocoderError) -> Self {
        match error {
            ShopGeocoderError::NotFound => Self::InvalidAddress,
            ShopGeocoderError::TemporarilyUnavailable => Self::TemporarilyUnavailable {
                source: static_error("temporary geocoding failure"),
            },
            ShopGeocoderError::Internal => Self::Internal {
                source: static_error("internal geocoding failure"),
            },
        }
    }
}

struct PreparedUpdateShopCommand {
    shop_id: ShopId,
    shop_type: PatchField<ShopType>,
    domains: PatchField<HashSet<Domain>>,
    shopify_domain: PatchField<Domain>,
    shopify_currency: PatchField<Currency>,
    shopify_language: PatchField<Language>,
    woocommerce_webhook_secret: PatchField<WoocommerceWebhookSecret>,
    woocommerce_currency: PatchField<Currency>,
    woocommerce_language: PatchField<Language>,
    url: PatchField<Url>,
    image: PatchField<Url>,
    structured_address: PatchField<ShopAddress>,
    phone: PatchField<String>,
    email: PatchField<Email>,
    affiliate_configuration: PatchField<AffiliateConfiguration>,
}

async fn prepare_update<G>(
    command: UpdateShopCommand,
    geocoder: &G,
) -> Result<PreparedUpdateShopCommand, UpdateShopError>
where
    G: ShopGeocoder,
{
    let UpdateShopCommand {
        shop_id,
        shop_type,
        domains,
        shopify_domain,
        shopify_currency,
        shopify_language,
        woocommerce_webhook_secret,
        woocommerce_currency,
        woocommerce_language,
        url,
        image,
        structured_address,
        phone,
        email,
        affiliate_configuration,
    } = command;

    let structured_address = match structured_address {
        PatchField::Unchanged => PatchField::Unchanged,
        PatchField::Clear => PatchField::Clear,
        PatchField::Set(structured) => {
            let geo = geocoder.geocode(&structured).await?;
            PatchField::Set(ShopAddress {
                structured,
                geo: Some(geo),
            })
        }
    };

    Ok(PreparedUpdateShopCommand {
        shop_id,
        shop_type,
        domains,
        shopify_domain,
        shopify_currency,
        shopify_language,
        woocommerce_webhook_secret,
        woocommerce_currency,
        woocommerce_language,
        url,
        image,
        structured_address,
        phone,
        email,
        affiliate_configuration,
    })
}

fn apply_update(
    shop: &mut Shop,
    command: PreparedUpdateShopCommand,
) -> Result<ChangeOutcome, UpdateShopError> {
    let PreparedUpdateShopCommand {
        shop_id: _,
        shop_type,
        domains,
        shopify_domain,
        shopify_currency,
        shopify_language,
        woocommerce_webhook_secret,
        woocommerce_currency,
        woocommerce_language,
        url,
        image,
        structured_address,
        phone,
        email,
        affiliate_configuration,
    } = command;

    let mut outcome = ChangeOutcome::Unchanged;

    outcome = outcome.combine(match shop_type {
        PatchField::Unchanged => ChangeOutcome::Unchanged,
        PatchField::Set(value) => shop.change_shop_type(value),
        PatchField::Clear => return Err(UpdateShopError::ShopTypeRequired),
    });

    outcome = outcome.combine(match domains {
        PatchField::Unchanged => ChangeOutcome::Unchanged,
        PatchField::Set(value) => shop.replace_domains(value),
        PatchField::Clear => return Err(UpdateShopError::DomainsRequired),
    });

    if let Some(shopify) = patch_shopify(
        shop.shopify().cloned(),
        shopify_domain,
        shopify_currency,
        shopify_language,
    )? {
        outcome = outcome.combine(shop.replace_shopify_integration(shopify));
    }

    if let Some(woocommerce) = patch_woocommerce(
        shop.woocommerce().cloned(),
        woocommerce_webhook_secret,
        woocommerce_currency,
        woocommerce_language,
    ) {
        outcome = outcome.combine(shop.replace_woocommerce_integration(woocommerce));
    }

    if url.is_changed() || image.is_changed() {
        let current = shop.presentation().clone();
        let presentation = ShopPresentation {
            url: apply_optional_patch(current.url, url),
            image: apply_optional_patch(current.image, image),
        };
        outcome = outcome.combine(shop.replace_presentation(presentation));
    }

    outcome = outcome.combine(match structured_address {
        PatchField::Unchanged => ChangeOutcome::Unchanged,
        PatchField::Clear => shop.replace_address(None),
        PatchField::Set(address) => shop.replace_address(Some(address)),
    });

    if phone.is_changed() || email.is_changed() {
        let current = shop.contact().clone();
        let contact = ShopContact {
            phone: apply_optional_patch(current.phone, phone),
            email: apply_optional_patch(current.email, email),
        };
        outcome = outcome.combine(shop.replace_contact(contact));
    }

    if affiliate_configuration.is_changed() {
        outcome = outcome.combine(shop.replace_affiliate_configuration(apply_optional_patch(
            shop.affiliate_configuration().cloned(),
            affiliate_configuration,
        )));
    }

    Ok(outcome)
}

fn patch_shopify(
    current: Option<ShopifyIntegration>,
    domain: PatchField<Domain>,
    currency: PatchField<Currency>,
    language: PatchField<Language>,
) -> Result<Option<Option<ShopifyIntegration>>, UpdateShopError> {
    if !domain.is_changed() && !currency.is_changed() && !language.is_changed() {
        return Ok(None);
    }

    let current_domain = current.as_ref().map(|value| value.domain.clone());
    let new_domain = apply_optional_patch(current_domain, domain);

    match new_domain {
        Some(domain) => Ok(Some(Some(ShopifyIntegration {
            domain,
            currency: apply_optional_patch(
                current.as_ref().and_then(|value| value.currency),
                currency,
            ),
            language: apply_optional_patch(
                current.as_ref().and_then(|value| value.language),
                language,
            ),
        }))),
        None if matches!(currency, PatchField::Set(_))
            || matches!(language, PatchField::Set(_)) =>
        {
            Err(UpdateShopError::ShopifyDomainRequired)
        }
        None => Ok(Some(None)),
    }
}

fn patch_woocommerce(
    current: Option<WoocommerceIntegration>,
    webhook_secret: PatchField<WoocommerceWebhookSecret>,
    currency: PatchField<Currency>,
    language: PatchField<Language>,
) -> Option<Option<WoocommerceIntegration>> {
    if !webhook_secret.is_changed() && !currency.is_changed() && !language.is_changed() {
        return None;
    }

    Some(woocommerce_integration(
        apply_optional_patch(
            current
                .as_ref()
                .and_then(|value| value.webhook_secret.clone()),
            webhook_secret,
        ),
        apply_optional_patch(current.as_ref().and_then(|value| value.currency), currency),
        apply_optional_patch(current.as_ref().and_then(|value| value.language), language),
    ))
}

fn apply_optional_patch<T>(current: Option<T>, patch: PatchField<T>) -> Option<T> {
    match patch {
        PatchField::Unchanged => current,
        PatchField::Set(value) => Some(value),
        PatchField::Clear => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ports::{
        ShopDetailsReader, ShopDetailsReaderFactory, ShopRepository, ShopRepositoryError,
        ShopRepositoryFactory, ShopStorageVersion, ShopWritePolicyError, VersionedShop,
    };
    use common::error::boxed::static_error;
    use common::operation_context::{CorrelationId, Principal, RequestId};
    use common::shop_name::ShopName;
    use common::shop_slug_id::ShopSlugId;
    use common::transaction::{TransactionError, UnitOfWork};
    use common::versioned::Versioned;
    use shop_core::address::GeoAddress;
    use shop_core::partner_status::ShopPartnerStatus;
    use shop_core::shop::NewShop;
    use std::sync::{Arc, Mutex};
    use time::OffsetDateTime;

    #[derive(Clone, Copy)]
    enum RepoErrorKind {
        SlugConflict,
        ConcurrencyConflict,
    }

    #[derive(Default)]
    struct Counts {
        begin: usize,
        commit: usize,
        find_by_id: usize,
        update: usize,
        geocode: usize,
    }

    #[derive(Default)]
    struct State {
        begin_error: bool,
        commit_error: bool,
        shop_by_id: Option<VersionedShop>,
        find_by_id_error: Option<RepoErrorKind>,
        update_error: Option<RepoErrorKind>,
        geocoder_error: Option<ShopGeocoderError>,
        updated: Option<Shop>,
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
    struct FakeDetailsReaderFactory {
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

    struct FakeDetailsReader {
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

    impl ShopDetailsReaderFactory<FakeTx> for FakeDetailsReaderFactory {
        fn in_transaction<'tx>(&'tx self, _tx: &'tx mut FakeTx) -> impl ShopDetailsReader + 'tx {
            FakeDetailsReader {
                state: Arc::clone(&self.state),
            }
        }
    }

    #[async_trait::async_trait]
    impl ShopRepository for FakeShopRepository {
        async fn find_by_id(
            &mut self,
            _id: ShopId,
        ) -> Result<Option<VersionedShop>, ShopRepositoryError> {
            with_state(&self.state, |state| {
                state.counts.find_by_id += 1;
                match state.find_by_id_error {
                    Some(kind) => Err(shop_repo_error(kind)),
                    None => Ok(state.shop_by_id.clone()),
                }
            })
        }

        async fn find_by_slug(
            &mut self,
            _slug_id: &ShopSlugId,
        ) -> Result<Option<VersionedShop>, ShopRepositoryError> {
            Ok(None)
        }

        async fn insert(&mut self, _shop: &Shop) -> Result<(), ShopRepositoryError> {
            Ok(())
        }

        async fn update(
            &mut self,
            shop: &Shop,
            _expected_version: ShopStorageVersion,
        ) -> Result<(), ShopRepositoryError> {
            with_state(&self.state, |state| {
                state.counts.update += 1;
                match state.update_error {
                    Some(kind) => Err(shop_repo_error(kind)),
                    None => {
                        state.updated = Some(shop.clone());
                        Ok(())
                    }
                }
            })
        }
    }

    #[async_trait::async_trait]
    impl ShopDetailsReader for FakeDetailsReader {
        async fn find_details(
            &mut self,
            _request: &GetShopRequest,
        ) -> Result<Option<ShopDetailsView>, ShopDetailsReadError> {
            Ok(with_state(&self.state, |state| {
                state
                    .updated
                    .as_ref()
                    .or_else(|| state.shop_by_id.as_ref().map(|versioned| &versioned.value))
                    .map(details_from_shop)
            }))
        }
    }

    #[async_trait::async_trait]
    impl ShopWritePolicy for AllowPolicy {
        async fn ensure_can_create_shop(
            &self,
            _context: &OperationContext,
        ) -> Result<(), ShopWritePolicyError> {
            Ok(())
        }

        async fn ensure_can_update_shop(
            &self,
            _context: &OperationContext,
            _shop_id: ShopId,
        ) -> Result<(), ShopWritePolicyError> {
            Ok(())
        }
    }

    #[async_trait::async_trait]
    impl ShopGeocoder for FakeGeocoder {
        async fn geocode(
            &self,
            _address: &StructuredAddress,
        ) -> Result<GeoAddress, ShopGeocoderError> {
            with_state(&self.state, |state| {
                state.counts.geocode += 1;
                match &state.geocoder_error {
                    Some(ShopGeocoderError::NotFound) => Err(ShopGeocoderError::NotFound),
                    Some(ShopGeocoderError::TemporarilyUnavailable) => {
                        Err(ShopGeocoderError::TemporarilyUnavailable)
                    }
                    Some(ShopGeocoderError::Internal) => Err(ShopGeocoderError::Internal),
                    None => Ok(GeoAddress { lat: 1.0, lon: 2.0 }),
                }
            })
        }
    }

    #[test]
    fn should_report_empty_update_when_all_fields_unchanged() {
        let command = UpdateShopCommand {
            shop_id: ShopId::new(),
            ..Default::default()
        };

        assert!(command.is_empty());
    }

    #[test]
    fn should_report_non_empty_update_when_field_set() {
        let command = UpdateShopCommand {
            shop_id: ShopId::new(),
            shop_type: PatchField::Set(ShopType::Marketplace),
            ..Default::default()
        };

        assert!(!command.is_empty());
    }

    #[test]
    fn should_report_non_empty_update_when_optional_field_cleared() {
        let command = UpdateShopCommand {
            shop_id: ShopId::new(),
            email: PatchField::Clear,
            ..Default::default()
        };

        assert!(!command.is_empty());
    }

    #[test]
    fn should_reject_shopify_settings_without_domain() {
        let result = patch_shopify(
            None,
            PatchField::Unchanged,
            PatchField::Set(Currency::Eur),
            PatchField::Unchanged,
        );

        assert!(matches!(
            result,
            Err(UpdateShopError::ShopifyDomainRequired)
        ));
    }

    #[tokio::test]
    async fn should_update_shop_when_patch_changes_fields() -> Result<(), Box<dyn std::error::Error>>
    {
        let state = shared_state();
        let existing = shop("Antik Markt");
        let shop_id = existing.id();
        with_state(&state, |state| {
            state.shop_by_id = Some(versioned_shop(existing))
        });
        let handler = build_handler(&state);
        let mut domains = HashSet::new();
        domains.insert(Domain::try_from("example.org")?);
        let command = UpdateShopCommand {
            shop_id,
            shop_type: PatchField::Set(ShopType::Marketplace),
            domains: PatchField::Set(domains),
            shopify_domain: PatchField::Set(Domain::try_from("shopify.example.org")?),
            shopify_currency: PatchField::Set(Currency::Usd),
            shopify_language: PatchField::Set(Language::De),
            woocommerce_webhook_secret: PatchField::Set(WoocommerceWebhookSecret::from("secret")),
            woocommerce_currency: PatchField::Set(Currency::Gbp),
            woocommerce_language: PatchField::Set(Language::Fr),
            url: PatchField::Set(Url::parse("https://example.org")?),
            image: PatchField::Set(Url::parse("https://example.org/image.png")?),
            structured_address: PatchField::Set(address()),
            phone: PatchField::Set("123".to_string()),
            email: PatchField::Unchanged,
            affiliate_configuration: PatchField::Set(AffiliateConfiguration::Partnerize {
                camref: "camref".to_string(),
            }),
        };

        let result = handler.execute(&system_context(), command).await;

        assert!(matches!(result, Ok(ref value) if value.shop_id == shop_id));
        assert_counts(&state, |counts| {
            assert_eq!(1, counts.geocode);
            assert_eq!(1, counts.update);
            assert_eq!(1, counts.commit);
        });
        let updated = with_state(&state, |state| state.updated.clone());
        assert!(matches!(updated, Some(ref shop) if shop.shop_type() == ShopType::Marketplace));
        Ok(())
    }

    #[tokio::test]
    async fn should_commit_without_update_when_update_noop() {
        let state = shared_state();
        let existing = shop("Antik Markt");
        let shop_id = existing.id();
        with_state(&state, |state| {
            state.shop_by_id = Some(versioned_shop(existing))
        });
        let handler = build_handler(&state);

        let result = handler
            .execute(
                &system_context(),
                UpdateShopCommand {
                    shop_id,
                    ..Default::default()
                },
            )
            .await;

        assert!(result.is_ok());
        assert_counts(&state, |counts| {
            assert_eq!(0, counts.update);
            assert_eq!(1, counts.commit);
        });
    }

    #[tokio::test]
    async fn should_not_commit_update_when_operation_fails() {
        let state = shared_state();
        let existing = shop("Antik Markt");
        let shop_id = existing.id();
        with_state(&state, |state| {
            state.shop_by_id = Some(versioned_shop(existing))
        });
        let handler = build_handler(&state);

        let result = handler
            .execute(
                &system_context(),
                UpdateShopCommand {
                    shop_id,
                    shop_type: PatchField::Clear,
                    ..Default::default()
                },
            )
            .await;

        assert!(matches!(result, Err(UpdateShopError::ShopTypeRequired)));
        assert_counts(&state, |counts| assert_eq!(0, counts.commit));
    }

    #[tokio::test]
    async fn should_cover_update_not_found_repo_geocoder_and_transaction_errors() {
        let state = shared_state();
        let handler = build_handler(&state);

        let not_found = handler
            .execute(
                &system_context(),
                UpdateShopCommand {
                    shop_id: ShopId::new(),
                    shop_type: PatchField::Set(ShopType::Marketplace),
                    ..Default::default()
                },
            )
            .await;

        assert!(matches!(not_found, Err(UpdateShopError::ShopNotFound)));
        assert_counts(&state, |counts| assert_eq!(0, counts.commit));

        let state = shared_state();
        with_state(&state, |state| state.begin_error = true);
        let handler = build_handler(&state);
        let begin = handler
            .execute(&system_context(), UpdateShopCommand::default())
            .await;
        assert!(matches!(
            begin,
            Err(UpdateShopError::BeginTransactionFailed)
        ));

        let state = shared_state();
        let existing = shop("Commit Fail");
        let shop_id = existing.id();
        with_state(&state, |state| {
            state.shop_by_id = Some(versioned_shop(existing));
            state.commit_error = true;
        });
        let handler = build_handler(&state);
        let commit = handler
            .execute(
                &system_context(),
                UpdateShopCommand {
                    shop_id,
                    shop_type: PatchField::Set(ShopType::Marketplace),
                    ..Default::default()
                },
            )
            .await;
        assert!(matches!(
            commit,
            Err(UpdateShopError::CommitTransactionFailed)
        ));

        let state = shared_state();
        with_state(&state, |state| {
            state.find_by_id_error = Some(RepoErrorKind::ConcurrencyConflict)
        });
        let handler = build_handler(&state);
        let repo = handler
            .execute(
                &system_context(),
                UpdateShopCommand {
                    shop_id: ShopId::new(),
                    shop_type: PatchField::Set(ShopType::Marketplace),
                    ..Default::default()
                },
            )
            .await;
        assert!(matches!(repo, Err(UpdateShopError::ConcurrencyConflict)));

        let state = shared_state();
        with_state(&state, |state| {
            state.geocoder_error = Some(ShopGeocoderError::Internal)
        });
        let handler = build_handler(&state);
        let geo = handler
            .execute(
                &system_context(),
                UpdateShopCommand {
                    shop_id: ShopId::new(),
                    structured_address: PatchField::Set(address()),
                    ..Default::default()
                },
            )
            .await;
        assert!(matches!(geo, Err(UpdateShopError::Internal { .. })));
        assert_counts(&state, |counts| assert_eq!(0, counts.begin));
    }

    #[tokio::test]
    async fn should_cover_update_patch_validation_branches() {
        let state = shared_state();
        let existing = shop("Patch Bad");
        let shop_id = existing.id();
        with_state(&state, |state| {
            state.shop_by_id = Some(versioned_shop(existing))
        });
        let handler = build_handler(&state);

        let domains = handler
            .execute(
                &system_context(),
                UpdateShopCommand {
                    shop_id,
                    domains: PatchField::Clear,
                    ..Default::default()
                },
            )
            .await;
        assert!(matches!(domains, Err(UpdateShopError::DomainsRequired)));

        let state = shared_state();
        let existing = shop("Patch Bad 2");
        let shop_id = existing.id();
        with_state(&state, |state| {
            state.shop_by_id = Some(versioned_shop(existing))
        });
        let handler = build_handler(&state);
        let shopify = handler
            .execute(
                &system_context(),
                UpdateShopCommand {
                    shop_id,
                    shopify_currency: PatchField::Set(Currency::Usd),
                    ..Default::default()
                },
            )
            .await;
        assert!(matches!(
            shopify,
            Err(UpdateShopError::ShopifyDomainRequired)
        ));
    }

    #[tokio::test]
    async fn should_map_slug_conflict_when_update_fails() {
        let state = shared_state();
        let existing = shop("Slug Error");
        let shop_id = existing.id();
        with_state(&state, |state| {
            state.shop_by_id = Some(versioned_shop(existing));
            state.update_error = Some(RepoErrorKind::SlugConflict);
        });
        let handler = build_handler(&state);

        let result = handler
            .execute(
                &system_context(),
                UpdateShopCommand {
                    shop_id,
                    shop_type: PatchField::Set(ShopType::Marketplace),
                    ..Default::default()
                },
            )
            .await;

        assert!(matches!(result, Err(UpdateShopError::SlugConflict { .. })));
        assert_counts(&state, |counts| assert_eq!(0, counts.commit));
    }

    fn build_handler(state: &Arc<Mutex<State>>) -> impl UpdateShopUseCase {
        UpdateShopHandler::new(
            uow(state),
            shop_repo(state),
            details_reader(state),
            geocoder(state),
            AllowPolicy,
        )
    }

    fn shop_repo(state: &Arc<Mutex<State>>) -> FakeShopRepositoryFactory {
        FakeShopRepositoryFactory {
            state: Arc::clone(state),
        }
    }

    fn details_reader(state: &Arc<Mutex<State>>) -> FakeDetailsReaderFactory {
        FakeDetailsReaderFactory {
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
            RepoErrorKind::SlugConflict => ShopRepositoryError::SlugConflict {
                source: static_error("slug conflict"),
            },
            RepoErrorKind::ConcurrencyConflict => ShopRepositoryError::ConcurrencyConflict,
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

    fn versioned_shop(shop: Shop) -> VersionedShop {
        Versioned::new(shop, ShopStorageVersion::INITIAL)
    }

    fn details_from_shop(shop: &Shop) -> ShopDetailsView {
        let now = OffsetDateTime::now_utc();
        ShopDetailsView {
            shop_id: shop.id(),
            shop_slug_id: shop.slug_id().clone(),
            name: shop.name().clone(),
            shop_type: shop.shop_type(),
            domains: shop.domains().clone(),
            shopify_domain: shop.shopify().map(|value| value.domain.clone()),
            shopify_currency: shop.shopify().and_then(|value| value.currency),
            shopify_language: shop.shopify().and_then(|value| value.language),
            woocommerce_currency: shop.woocommerce().and_then(|value| value.currency),
            woocommerce_language: shop.woocommerce().and_then(|value| value.language),
            url: shop.presentation().url.clone(),
            view_url: shop.view_url(),
            image: shop.presentation().image.clone(),
            structured_address: shop.address().map(|value| value.structured.clone()),
            geo_address: shop.address().and_then(|value| value.geo),
            phone: shop.contact().phone.clone(),
            email: shop.contact().email.clone(),
            partner_status: shop.partner_status(),
            affiliate_configuration: shop.affiliate_configuration().cloned(),
            created: now,
            updated: now,
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
