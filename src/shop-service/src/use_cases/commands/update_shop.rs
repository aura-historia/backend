use crate::ports::{
    ShopGeocoder, ShopGeocoderError, ShopRepository, ShopRepositoryError, ShopRepositoryFactory,
};
use crate::use_cases::commands::create_shop::woocommerce_integration;
use common::change_outcome::ChangeOutcome;
use common::currency::domain::Currency;
use common::domain::Domain;
use common::language::domain::Language;
use common::operation_context::OperationContext;
use common::patch_field::PatchField;
use common::transaction::{Transaction, UnitOfWork};
use common::write_metadata::WriteMetadata;
use common::{shop_id::ShopId, shop_name::ShopName, shop_slug_id::ShopSlugId};
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

#[derive(Debug, Clone, PartialEq)]
pub struct UpdateShopResult {
    pub shop_id: ShopId,
    pub shop_slug_id: ShopSlugId,
    pub name: ShopName,
}

#[derive(Debug, thiserror::Error)]
pub enum UpdateShopError {
    #[error("authenticated actor required to update shop")]
    AuthenticatedActorRequired,
    #[error("shop not found")]
    ShopNotFound,
    #[error("concurrent shop update")]
    ConcurrencyConflict,
    #[error("shop slug already exists")]
    SlugConflict,
    #[error("shop type is required")]
    ShopTypeRequired,
    #[error("shop domains are required")]
    DomainsRequired,
    #[error("shopify domain is required when changing shopify settings")]
    ShopifyDomainRequired,
    #[error("invalid shop address")]
    InvalidAddress,
    #[error("temporary shop persistence failure")]
    TemporarilyUnavailable,
    #[error("invalid persisted shop state")]
    InvalidPersistedState,
    #[error("internal shop persistence failure")]
    Internal,
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

pub struct UpdateShopHandler<U, R, G> {
    unit_of_work: U,
    shops: R,
    geocoder: G,
}

impl<U, R, G> UpdateShopHandler<U, R, G> {
    pub fn new(unit_of_work: U, shops: R, geocoder: G) -> Self {
        Self {
            unit_of_work,
            shops,
            geocoder,
        }
    }
}

#[async_trait::async_trait]
impl<U, R, G> UpdateShopUseCase for UpdateShopHandler<U, R, G>
where
    U: UnitOfWork,
    R: ShopRepositoryFactory<U::Tx>,
    G: ShopGeocoder,
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
        let metadata = WriteMetadata::try_from(context)
            .map_err(|_| UpdateShopError::AuthenticatedActorRequired)?;
        tracing::Span::current().record("actor_id", tracing::field::display(metadata.actor()));

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
                .update(&shop, version, &metadata)
                .await?;
        }

        tx.commit()
            .await
            .map_err(|_| UpdateShopError::CommitTransactionFailed)?;

        tracing::info!(
            event = "shop.updated",
            actor_type = context.principal.kind(),
            actor_id = %metadata.actor(),
            shop_id = %shop.id(),
            shop_slug_id = %shop.slug_id(),
            changed = outcome.changed(),
            outcome = "success",
        );

        Ok(UpdateShopResult::from(&shop))
    }
}

impl From<&Shop> for UpdateShopResult {
    fn from(shop: &Shop) -> Self {
        Self {
            shop_id: shop.id(),
            shop_slug_id: shop.slug_id().clone(),
            name: shop.name().clone(),
        }
    }
}

impl From<ShopRepositoryError> for UpdateShopError {
    fn from(error: ShopRepositoryError) -> Self {
        match error {
            ShopRepositoryError::ConcurrencyConflict => Self::ConcurrencyConflict,
            ShopRepositoryError::SlugConflict => Self::SlugConflict,
            ShopRepositoryError::TemporarilyUnavailable => Self::TemporarilyUnavailable,
            ShopRepositoryError::InvalidPersistedState => Self::InvalidPersistedState,
            ShopRepositoryError::Internal => Self::Internal,
        }
    }
}

impl From<ShopGeocoderError> for UpdateShopError {
    fn from(error: ShopGeocoderError) -> Self {
        match error {
            ShopGeocoderError::NotFound => Self::InvalidAddress,
            ShopGeocoderError::TemporarilyUnavailable => Self::TemporarilyUnavailable,
            ShopGeocoderError::Internal => Self::Internal,
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
}
