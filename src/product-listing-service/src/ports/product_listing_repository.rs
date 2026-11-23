#![allow(dead_code)]

use application::error::BoxError;
use domain_primitives::event_id::EventId;
use domain_primitives::versioned::Versioned;
use product_listing_core::{
    product_listing::ProductListing,
    product_listing_event::ProductListingEventPayload,
    product_listing_id::{ProductListingId, ProductListingKey},
};

domain_primitives::version_newtype!(ProductListingStorageVersion, no_serde);

pub type VersionedProductListing = Versioned<ProductListing, ProductListingStorageVersion>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ProductListingWriteEffects {
    pub content_source: ProductListingContentSourceEffect,
    pub advance_embedding_source: bool,
}

/// How a write moves the content revision that content assessment and title translation work on
/// (docs/product-listing.md#title-translation).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ProductListingContentSourceEffect {
    /// Neither title nor description changed.
    #[default]
    Keep,
    /// Only the description changed. Translations of the replaced revision, and translation work
    /// still pending for it, move onto the new revision because its title is the same.
    AdvanceKeepingTitle,
    /// A discovery or a title change. Translations of earlier revisions are discarded.
    AdvanceWithNewTitle,
}

impl ProductListingContentSourceEffect {
    pub const fn advances(self) -> bool {
        !matches!(self, Self::Keep)
    }
}

impl From<&ProductListingEventPayload> for ProductListingWriteEffects {
    fn from(payload: &ProductListingEventPayload) -> Self {
        match payload {
            ProductListingEventPayload::Discovered(_) => Self {
                content_source: ProductListingContentSourceEffect::AdvanceWithNewTitle,
                advance_embedding_source: true,
            },
            ProductListingEventPayload::Changed(changes) => Self {
                content_source: if changes.title().is_some() {
                    ProductListingContentSourceEffect::AdvanceWithNewTitle
                } else if changes.description().is_some() {
                    ProductListingContentSourceEffect::AdvanceKeepingTitle
                } else {
                    ProductListingContentSourceEffect::Keep
                },
                advance_embedding_source: changes.has_text_change()
                    || changes.image_count().is_some(),
            },
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ProductListingRepositoryError {
    #[error("temporary product listing persistence failure")]
    TemporarilyUnavailable {
        #[source]
        source: BoxError,
    },
    #[error("product listing query failed")]
    QueryFailed {
        #[source]
        source: BoxError,
    },
    #[error("product storage version did not match expected version")]
    ConcurrencyConflict,
    #[error("product already exists for source listing identity")]
    SourceListingAlreadyExists,
    #[error("product slug already exists")]
    ProductListingTitleSlugAlreadyExists,
    #[error("product lookup by id failed")]
    ProductListingLookupByIdFailed,
    #[error("product lookup by source listing identity failed")]
    ProductListingLookupByKeyFailed {
        #[source]
        source: BoxError,
    },
    #[error("product insert failed")]
    ProductListingInsertFailed,
    #[error("product update failed")]
    ProductListingUpdateFailed,
    #[error("persisted product slug is invalid")]
    InvalidProductListingSlugPersisted,
    #[error("persisted source listing ID is invalid")]
    InvalidSourceListingIdPersisted,
    #[error("persisted title is incomplete")]
    IncompleteTitlePersisted,
    #[error("persisted title language is invalid")]
    InvalidTitleLanguagePersisted,
    #[error("persisted description is incomplete")]
    IncompleteDescriptionPersisted,
    #[error("persisted description language is invalid")]
    InvalidDescriptionLanguagePersisted,
    #[error("persisted product listing price state is invalid")]
    InvalidProductListingPricePersisted,
    #[error("persisted product listing price kind is invalid")]
    InvalidProductListingPriceKindPersisted,
    #[error("persisted price is incomplete")]
    IncompletePricePersisted,
    #[error("persisted price amount is negative")]
    NegativePriceAmountPersisted,
    #[error("persisted price currency is invalid")]
    InvalidPriceCurrencyPersisted,
    #[error("persisted listing availability is invalid")]
    InvalidListingAvailabilityPersisted,
    #[error("persisted listing lifecycle is invalid")]
    InvalidListingLifecyclePersisted,
    #[error("persisted product URL is invalid")]
    InvalidProductListingUrlPersisted,
    #[error("persisted product images value is invalid")]
    InvalidProductListingImagesPersisted,
    #[error("persisted product image URL is invalid")]
    InvalidProductListingImageUrlPersisted,

    #[error("persisted aggregate state is invalid")]
    InvalidAggregateStatePersisted,
    #[error("invalid persisted listing delivery configuration")]
    InvalidDeliveryPersisted {
        #[source]
        source: BoxError,
    },
}

#[async_trait::async_trait]
pub trait ProductListingRepository: Send {
    async fn find_by_id(
        &mut self,
        id: ProductListingId,
    ) -> Result<Option<VersionedProductListing>, ProductListingRepositoryError>;

    /// Restrict the lookup before decoding persisted state. A listing outside the
    /// source is absent even if its persisted state cannot be rehydrated.
    async fn find_by_id_in_source(
        &mut self,
        source: listing_source_core::ListingSourceId,
        id: ProductListingId,
    ) -> Result<Option<VersionedProductListing>, ProductListingRepositoryError>;

    async fn find_by_key(
        &mut self,
        key: &ProductListingKey,
    ) -> Result<Option<VersionedProductListing>, ProductListingRepositoryError>;

    async fn insert(
        &mut self,
        product: &ProductListing,
        current_event_id: EventId,
    ) -> Result<VersionedProductListing, ProductListingRepositoryError>;

    async fn update(
        &mut self,
        product: &ProductListing,
        expected_version: ProductListingStorageVersion,
        current_event_id: EventId,
        effects: ProductListingWriteEffects,
    ) -> Result<VersionedProductListing, ProductListingRepositoryError>;
}

pub trait ProductListingRepositoryFactory<Tx>: Send + Sync {
    fn in_transaction<'tx>(&'tx self, tx: &'tx mut Tx) -> impl ProductListingRepository + 'tx;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::use_cases::{
        CreateProductListingError, UpdateProductListingError, UpsertProductListingError,
        WithdrawProductListingError,
    };
    use application::error::box_error;
    use localization::{Language, Localized};
    use product_listing_core::{
        description::Description,
        product_listing_event::{ProductListingTextTransition, RehydratedProductListingChanged},
        title::Title,
    };

    fn text<T: From<&'static str>>(value: &'static str) -> Option<Localized<Language, T>> {
        Some(Localized::new(Language::De, T::from(value)))
    }

    fn changed(
        title: Option<ProductListingTextTransition<Title>>,
        description: Option<ProductListingTextTransition<Description>>,
    ) -> ProductListingEventPayload {
        ProductListingEventPayload::rehydrate_changed(RehydratedProductListingChanged {
            title,
            description,
            price: None,
            price_estimate_min: None,
            price_estimate_max: None,
            availability: None,
            url: Some((
                "https://shop.example/a".parse().unwrap(),
                "https://shop.example/b".parse().unwrap(),
            )),
            images: None,
            auction: None,
            lifecycle: None,
            sale_observation: None,
            delivery: None,
        })
        .unwrap()
    }

    fn content_source(payload: &ProductListingEventPayload) -> ProductListingContentSourceEffect {
        ProductListingWriteEffects::from(payload).content_source
    }

    #[test]
    fn should_keep_title_when_only_the_description_changes() {
        let payload = changed(None, Some((text("alt"), text("neu"))));

        assert_eq!(
            ProductListingContentSourceEffect::AdvanceKeepingTitle,
            content_source(&payload)
        );
    }

    #[test]
    fn should_advance_with_new_title_when_the_title_changes() {
        let title_only = changed(Some((text("Vase"), text("Krug"))), None);
        let both = changed(
            Some((text("Vase"), text("Krug"))),
            Some((text("alt"), text("neu"))),
        );
        let language_only = changed(
            Some((
                text("Vase"),
                Some(Localized::new(Language::En, Title::from("Vase"))),
            )),
            None,
        );

        for payload in [title_only, both, language_only] {
            assert_eq!(
                ProductListingContentSourceEffect::AdvanceWithNewTitle,
                content_source(&payload)
            );
        }
    }

    #[test]
    fn should_keep_content_source_when_no_text_changes() {
        assert_eq!(
            ProductListingContentSourceEffect::Keep,
            content_source(&changed(None, None))
        );
    }

    #[test]
    fn ordinary_listing_commands_preserve_delivery_corruption_causes() {
        let invalid = || ProductListingRepositoryError::InvalidDeliveryPersisted {
            source: box_error(std::io::Error::other("synthetic delivery corruption")),
        };
        let errors = [
            box_error(CreateProductListingError::from(invalid())),
            box_error(UpdateProductListingError::from(invalid())),
            box_error(UpsertProductListingError::from(invalid())),
            box_error(WithdrawProductListingError::from(invalid())),
        ];
        for error in errors {
            let cause = error.source().unwrap();
            assert!(cause.is::<std::io::Error>());
            assert_eq!("synthetic delivery corruption", cause.to_string());
        }
    }
}
