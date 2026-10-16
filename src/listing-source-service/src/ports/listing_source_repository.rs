use application::{error::BoxError, patch_field::PatchField};
use listing_source_core::{
    Domain, ListingIngestionMethod, ListingSource, ListingSourceId, ListingSourceSlugId,
    WoocommerceWebhookSecret,
};
use localization::Language;
use money::Currency;
use time::OffsetDateTime;

domain_primitives::version_newtype!(ListingSourceStorageVersion);

#[derive(Debug, Clone, PartialEq)]
pub enum ListingIngestionConfiguration {
    WebCrawl {
        fallback_currency: Option<Currency>,
    },
    Shopify {
        domain: Domain,
        currency: Option<Currency>,
        language: Option<Language>,
    },
    Woocommerce {
        webhook_secret: WoocommerceWebhookSecret,
        currency: Option<Currency>,
        language: Option<Language>,
    },
    PartnerApi,
}

impl ListingIngestionConfiguration {
    #[allow(non_upper_case_globals)]
    pub const WebCrawl: Self = Self::WebCrawl {
        fallback_currency: None,
    };

    pub fn method(&self) -> ListingIngestionMethod {
        match self {
            Self::WebCrawl { .. } => ListingIngestionMethod::WebCrawl,
            Self::Shopify { .. } => ListingIngestionMethod::Shopify,
            Self::Woocommerce { .. } => ListingIngestionMethod::Woocommerce,
            Self::PartnerApi => ListingIngestionMethod::PartnerApi,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum ListingIngestionConfigurationUpdate {
    Configuration(ListingIngestionConfiguration),
    Woocommerce {
        webhook_secret: PatchField<WoocommerceWebhookSecret>,
        currency: Option<Currency>,
        language: Option<Language>,
    },
}

impl ListingIngestionConfigurationUpdate {
    pub fn method(&self) -> ListingIngestionMethod {
        match self {
            Self::Configuration(configuration) => configuration.method(),
            Self::Woocommerce { .. } => ListingIngestionMethod::Woocommerce,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct UpdateListingSourceIngestionConfigurations(pub Vec<ListingIngestionConfigurationUpdate>);

impl UpdateListingSourceIngestionConfigurations {
    pub fn resolve(
        &self,
        existing: &ListingSourceIngestionConfigurations,
    ) -> Result<ListingSourceIngestionConfigurations, ListingIngestionConfigurationMismatch> {
        let mut configurations = Vec::with_capacity(self.0.len());
        for update in &self.0 {
            match update {
                ListingIngestionConfigurationUpdate::Configuration(configuration) => {
                    if matches!(
                        configuration,
                        ListingIngestionConfiguration::Woocommerce { .. }
                    ) {
                        return Err(ListingIngestionConfigurationMismatch);
                    }
                    configurations.push(configuration.clone());
                }
                ListingIngestionConfigurationUpdate::Woocommerce {
                    webhook_secret,
                    currency,
                    language,
                } => {
                    let secret = match webhook_secret {
                        PatchField::Unchanged => {
                            existing
                                .0
                                .iter()
                                .find_map(|configuration| match configuration {
                                    ListingIngestionConfiguration::Woocommerce {
                                        webhook_secret,
                                        ..
                                    } => Some(webhook_secret.clone()),
                                    _ => None,
                                })
                        }
                        PatchField::Set(secret) => Some(secret.clone()),
                        PatchField::Clear => None,
                    }
                    .ok_or(ListingIngestionConfigurationMismatch)?;
                    configurations.push(ListingIngestionConfiguration::Woocommerce {
                        webhook_secret: secret,
                        currency: *currency,
                        language: *language,
                    });
                }
            }
        }
        let configurations = ListingSourceIngestionConfigurations(configurations);
        configurations.methods()?;
        Ok(configurations)
    }
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct ListingSourceIngestionConfigurations(pub Vec<ListingIngestionConfiguration>);

impl ListingSourceIngestionConfigurations {
    pub fn methods(
        &self,
    ) -> Result<
        std::collections::HashSet<ListingIngestionMethod>,
        ListingIngestionConfigurationMismatch,
    > {
        let methods = self
            .0
            .iter()
            .map(ListingIngestionConfiguration::method)
            .collect::<std::collections::HashSet<_>>();
        if methods.len() == self.0.len() {
            Ok(methods)
        } else {
            Err(ListingIngestionConfigurationMismatch)
        }
    }

    pub fn validate_for(
        &self,
        source: &ListingSource,
    ) -> Result<(), ListingIngestionConfigurationMismatch> {
        (self.methods()? == *source.ingestion_methods())
            .then_some(())
            .ok_or(ListingIngestionConfigurationMismatch)
    }

    pub fn replace_provider_configuration(
        &mut self,
        configuration: ListingIngestionConfiguration,
    ) -> Result<bool, ListingIngestionConfigurationMismatch> {
        let method = configuration.method();
        if !matches!(
            method,
            ListingIngestionMethod::Shopify | ListingIngestionMethod::Woocommerce
        ) {
            return Err(ListingIngestionConfigurationMismatch);
        }
        if let Some(existing) = self
            .0
            .iter_mut()
            .find(|existing| existing.method() == method)
        {
            let changed = *existing != configuration;
            *existing = configuration;
            Ok(changed)
        } else {
            self.0.push(configuration);
            Ok(true)
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("ingestion methods and configuration do not match")]
pub struct ListingIngestionConfigurationMismatch;

#[derive(Debug, Clone, PartialEq)]
pub struct StoredListingSource {
    pub source: ListingSource,
    pub configuration: ListingSourceIngestionConfigurations,
    pub version: ListingSourceStorageVersion,
    pub created: OffsetDateTime,
    pub updated: OffsetDateTime,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ListingSourceDeletionBlocker {
    Auctions,
    ProductListings,
    RawStreams,
    ApprovedPartnershipApplication,
    ExistingSourcePartnershipApplication,
}

#[derive(Debug, thiserror::Error)]
pub enum ListingSourceRepositoryError {
    #[error("concurrent listing source update")]
    ConcurrencyConflict,
    #[error("listing source slug conflict")]
    SlugConflict {
        #[source]
        source: BoxError,
    },
    #[error("listing source Shopify domain conflict")]
    ShopifyDomainConflict {
        #[source]
        source: BoxError,
    },
    #[error("temporary listing source persistence failure")]
    TemporarilyUnavailable {
        #[source]
        source: BoxError,
    },
    #[error("invalid persisted listing source state")]
    InvalidPersistedState {
        #[source]
        source: BoxError,
    },
    #[error("internal listing source persistence failure")]
    Internal {
        #[source]
        source: BoxError,
    },
}

#[async_trait::async_trait]
pub trait ListingSourceRepository: Send {
    async fn find_by_id(
        &mut self,
        id: ListingSourceId,
    ) -> Result<Option<StoredListingSource>, ListingSourceRepositoryError>;
    async fn find_by_slug(
        &mut self,
        slug: &ListingSourceSlugId,
    ) -> Result<Option<StoredListingSource>, ListingSourceRepositoryError>;
    /// Locks the source row through the enclosing transaction so protected-reference
    /// writers using FK or proposal locks serialize with deletion.
    async fn find_by_id_for_update(
        &mut self,
        id: ListingSourceId,
    ) -> Result<Option<StoredListingSource>, ListingSourceRepositoryError>;
    async fn find_deletion_blocker(
        &mut self,
        id: ListingSourceId,
    ) -> Result<Option<ListingSourceDeletionBlocker>, ListingSourceRepositoryError>;
    /// Explicitly removes source-owned configuration and grants, then deletes the
    /// locked aggregate using its loaded version.
    async fn delete_unused(
        &mut self,
        id: ListingSourceId,
        expected: ListingSourceStorageVersion,
    ) -> Result<(), ListingSourceRepositoryError>;
    async fn insert(
        &mut self,
        source: &ListingSource,
        configuration: &ListingSourceIngestionConfigurations,
    ) -> Result<StoredListingSource, ListingSourceRepositoryError>;
    async fn update(
        &mut self,
        source: &ListingSource,
        configuration: &ListingSourceIngestionConfigurations,
        expected: ListingSourceStorageVersion,
    ) -> Result<StoredListingSource, ListingSourceRepositoryError>;
}

pub trait ListingSourceRepositoryFactory<Tx>: Send + Sync {
    fn in_transaction<'tx>(&'tx self, tx: &'tx mut Tx) -> impl ListingSourceRepository + 'tx;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn secret(value: &str) -> WoocommerceWebhookSecret {
        WoocommerceWebhookSecret::try_from(value)
            .unwrap_or_else(|error| panic!("invalid test secret: {error}"))
    }

    #[test]
    fn update_replacement_preserves_omitted_woocommerce_secret() {
        let existing = ListingSourceIngestionConfigurations(vec![
            ListingIngestionConfiguration::Woocommerce {
                webhook_secret: secret("old-secret"),
                currency: Some(Currency::Eur),
                language: Language::from_code("de"),
            },
        ]);
        let update = UpdateListingSourceIngestionConfigurations(vec![
            ListingIngestionConfigurationUpdate::Woocommerce {
                webhook_secret: PatchField::Unchanged,
                currency: Some(Currency::Usd),
                language: Language::from_code("en"),
            },
        ]);

        let resolved = update
            .resolve(&existing)
            .unwrap_or_else(|error| panic!("resolve WooCommerce update: {error}"));
        assert_eq!(
            ListingSourceIngestionConfigurations(vec![
                ListingIngestionConfiguration::Woocommerce {
                    webhook_secret: secret("old-secret"),
                    currency: Some(Currency::Usd),
                    language: Language::from_code("en"),
                }
            ]),
            resolved
        );
    }

    #[test]
    fn update_requires_secret_when_adding_woocommerce_and_rejects_clear() {
        let omitted = UpdateListingSourceIngestionConfigurations(vec![
            ListingIngestionConfigurationUpdate::Woocommerce {
                webhook_secret: PatchField::Unchanged,
                currency: None,
                language: None,
            },
        ]);
        assert!(
            omitted
                .resolve(&ListingSourceIngestionConfigurations::default())
                .is_err()
        );

        let existing = ListingSourceIngestionConfigurations(vec![
            ListingIngestionConfiguration::Woocommerce {
                webhook_secret: secret("old-secret"),
                currency: None,
                language: None,
            },
        ]);
        let cleared = UpdateListingSourceIngestionConfigurations(vec![
            ListingIngestionConfigurationUpdate::Woocommerce {
                webhook_secret: PatchField::Clear,
                currency: None,
                language: None,
            },
        ]);
        assert!(cleared.resolve(&existing).is_err());
    }
}
