use crate::{
    ports::{
        ProductListingEventAppender, ProductListingEventAppenderFactory, ProductListingRepository,
        ProductListingRepositoryError, ProductListingRepositoryFactory, ProductListingWriteEffects,
        stamp_product_listing_event,
    },
    product_listing_title_slug_creation::{
        ProductListingTitleSlugGenerator, RandomProductListingTitleSlugGenerator,
    },
};
use application::{
    error::{BoxError, box_error},
    patch_field::PatchField,
};
use domain_primitives::{change_outcome::ChangeOutcome, event_id::EventId};
use indexmap::IndexSet;
use listing_source_core::ListingSourceId;
use localization::{Language, Localized};
use money::Price;
use product_listing_core::{
    description::Description,
    listing_availability::ListingAvailability,
    product_listing::{NewProductListing, ProductListing, ProductListingPricing},
    product_listing_id::ProductListingId,
    product_listing_image::ProductListingImage,
    product_listing_price::ProductListingPrice,
    source_listing_id::SourceListingId,
    title::Title,
};
use time::OffsetDateTime;
use url::Url;

/// Raw normalization owns no Auction interpretation. This intent changes only generic listing facts.
#[derive(Debug, Clone, PartialEq)]
pub struct CanonicalProductListingNonAuctionUpsert {
    pub listing_source_id: ListingSourceId,
    pub source_listing_id: SourceListingId,
    pub title: PatchField<Localized<Language, Title>>,
    pub description: PatchField<Localized<Language, Description>>,
    pub price: PatchField<ProductListingPrice>,
    pub price_estimate_min: PatchField<Price>,
    pub price_estimate_max: PatchField<Price>,
    pub availability: PatchField<ListingAvailability>,
    pub url: PatchField<Url>,
    pub images: PatchField<IndexSet<ProductListingImage>>,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CanonicalProductListingWriteResult {
    pub product_listing_id: ProductListingId,
    pub product_listing_event_id: Option<EventId>,
    pub outcome: ChangeOutcome,
}
#[derive(Debug, thiserror::Error)]
pub enum CanonicalProductListingWriteError {
    #[error("bound product listing was not found")]
    BoundProductListingNotFound,
    #[error("bound product listing identity does not match raw stream identity")]
    BoundProductListingIdentityMismatch,
    #[error("canonical product listing input is invalid")]
    InvalidInput {
        #[source]
        source: BoxError,
    },
    #[error("canonical product listing persistence failed")]
    Persistence {
        #[source]
        source: BoxError,
    },
    #[error("canonical product listing event append failed")]
    EventAppend {
        #[source]
        source: BoxError,
    },
}
pub struct CanonicalProductListingWriter;
impl CanonicalProductListingWriter {
    pub async fn upsert_non_auction_in_transaction<Tx, R, E>(
        tx: &mut Tx,
        products: &R,
        events: &E,
        bound_product_listing_id: Option<ProductListingId>,
        command: CanonicalProductListingNonAuctionUpsert,
    ) -> Result<CanonicalProductListingWriteResult, CanonicalProductListingWriteError>
    where
        R: ProductListingRepositoryFactory<Tx>,
        E: ProductListingEventAppenderFactory<Tx>,
    {
        let existing = match bound_product_listing_id {
            Some(id) => {
                let found = {
                    let mut repository = products.in_transaction(tx);
                    repository
                        .find_by_id(id)
                        .await
                        .map_err(map_repository_error)?
                };
                found.ok_or(CanonicalProductListingWriteError::BoundProductListingNotFound)?
            }
            None => {
                let key = product_listing_core::product_listing_id::ProductListingKey::new(
                    command.listing_source_id,
                    command.source_listing_id.clone(),
                );
                let found = {
                    let mut repository = products.in_transaction(tx);
                    repository
                        .find_by_key(&key)
                        .await
                        .map_err(map_repository_error)?
                };
                match found {
                    Some(value) => value,
                    None => return Self::create(tx, products, events, command).await,
                }
            }
        };
        if existing.value.listing_source_id() != command.listing_source_id
            || existing.value.source_listing_id() != &command.source_listing_id
        {
            return Err(CanonicalProductListingWriteError::BoundProductListingIdentityMismatch);
        };
        let mut product = existing.value;
        product
            .restore()
            .map_err(|e| CanonicalProductListingWriteError::InvalidInput {
                source: box_error(e),
            })?;
        apply_update(&mut product, &command)?;
        let event = product.take_pending_event_payload().map(|payload| {
            stamp_product_listing_event(product.id(), OffsetDateTime::now_utc(), payload)
        });
        let event_id = event.as_ref().map(|value| value.event_id);
        if let Some(event) = event {
            let effects = ProductListingWriteEffects::from(&event.payload);
            products
                .in_transaction(tx)
                .update(&product, existing.version, event.event_id, effects)
                .await
                .map_err(map_repository_error)?;
            events
                .in_transaction(tx)
                .append(&event)
                .await
                .map_err(|e| CanonicalProductListingWriteError::EventAppend {
                    source: box_error(e),
                })?;
        }
        Ok(CanonicalProductListingWriteResult {
            product_listing_id: product.id(),
            product_listing_event_id: event_id,
            outcome: if event_id.is_some() {
                ChangeOutcome::Changed
            } else {
                ChangeOutcome::Unchanged
            },
        })
    }
    pub async fn withdraw_in_transaction<Tx, R, E>(
        tx: &mut Tx,
        products: &R,
        events: &E,
        product_listing_id: ProductListingId,
    ) -> Result<CanonicalProductListingWriteResult, CanonicalProductListingWriteError>
    where
        R: ProductListingRepositoryFactory<Tx>,
        E: ProductListingEventAppenderFactory<Tx>,
    {
        let stored = {
            let mut repository = products.in_transaction(tx);
            repository
                .find_by_id(product_listing_id)
                .await
                .map_err(map_repository_error)?
        }
        .ok_or(CanonicalProductListingWriteError::BoundProductListingNotFound)?;
        let mut product = stored.value;
        let outcome = product.withdraw().map_err(|error| {
            CanonicalProductListingWriteError::InvalidInput {
                source: box_error(error),
            }
        })?;
        let event = product.take_pending_event_payload().map(|payload| {
            stamp_product_listing_event(product.id(), OffsetDateTime::now_utc(), payload)
        });
        let event_id = event.as_ref().map(|event| event.event_id);
        if let Some(event) = event {
            let effects = ProductListingWriteEffects::from(&event.payload);
            products
                .in_transaction(tx)
                .update(&product, stored.version, event.event_id, effects)
                .await
                .map_err(map_repository_error)?;
            events
                .in_transaction(tx)
                .append(&event)
                .await
                .map_err(|error| CanonicalProductListingWriteError::EventAppend {
                    source: box_error(error),
                })?;
        }
        Ok(CanonicalProductListingWriteResult {
            product_listing_id: product.id(),
            product_listing_event_id: event_id,
            outcome,
        })
    }

    async fn create<Tx, R, E>(
        tx: &mut Tx,
        products: &R,
        events: &E,
        command: CanonicalProductListingNonAuctionUpsert,
    ) -> Result<CanonicalProductListingWriteResult, CanonicalProductListingWriteError>
    where
        R: ProductListingRepositoryFactory<Tx>,
        E: ProductListingEventAppenderFactory<Tx>,
    {
        let url = match &command.url {
            PatchField::Set(url) => url.clone(),
            PatchField::Unchanged | PatchField::Clear => {
                return Err(CanonicalProductListingWriteError::InvalidInput {
                    source: box_error(std::io::Error::other("new raw listing requires URL")),
                });
            }
        };
        let title = patch_value(command.title);
        let title_slug_id = RandomProductListingTitleSlugGenerator
            .generate(
                title
                    .as_ref()
                    .map_or("listing", |value| value.payload.as_ref()),
            )
            .map_err(|e| CanonicalProductListingWriteError::InvalidInput {
                source: box_error(e),
            })?;
        let mut product = ProductListing::create(NewProductListing {
            id: ProductListingId::new(),
            title_slug_id,
            listing_source_id: command.listing_source_id,
            source_listing_id: command.source_listing_id,
            title,
            description: patch_value(command.description),
            pricing: ProductListingPricing {
                price: patch_value(command.price),
                price_estimate_min: patch_value(command.price_estimate_min),
                price_estimate_max: patch_value(command.price_estimate_max),
            },
            availability: patch_value(command.availability),
            url,
            images: match command.images {
                PatchField::Set(images) => images,
                PatchField::Unchanged | PatchField::Clear => IndexSet::new(),
            },
            auction: None,
        })
        .map_err(|e| CanonicalProductListingWriteError::InvalidInput {
            source: box_error(e),
        })?;
        let event = stamp_product_listing_event(
            product.id(),
            OffsetDateTime::now_utc(),
            product.take_pending_event_payload().ok_or_else(|| {
                CanonicalProductListingWriteError::InvalidInput {
                    source: box_error(std::io::Error::other(
                        "new listing did not produce discovery event",
                    )),
                }
            })?,
        );
        products
            .in_transaction(tx)
            .insert(&product, event.event_id)
            .await
            .map_err(map_repository_error)?;
        events
            .in_transaction(tx)
            .append(&event)
            .await
            .map_err(|e| CanonicalProductListingWriteError::EventAppend {
                source: box_error(e),
            })?;
        Ok(CanonicalProductListingWriteResult {
            product_listing_id: product.id(),
            product_listing_event_id: Some(event.event_id),
            outcome: ChangeOutcome::Changed,
        })
    }
}
fn apply_update(
    product: &mut ProductListing,
    command: &CanonicalProductListingNonAuctionUpsert,
) -> Result<(), CanonicalProductListingWriteError> {
    let mut pricing = product.pricing();
    apply_option(&mut pricing.price, command.price.clone());
    apply_option(
        &mut pricing.price_estimate_min,
        command.price_estimate_min.clone(),
    );
    apply_option(
        &mut pricing.price_estimate_max,
        command.price_estimate_max.clone(),
    );
    product.replace_pricing(pricing).map_err(|e| {
        CanonicalProductListingWriteError::InvalidInput {
            source: box_error(e),
        }
    })?;
    match command.availability.clone() {
        PatchField::Unchanged => {}
        PatchField::Set(v) => {
            product.set_availability(v).map_err(|e| {
                CanonicalProductListingWriteError::InvalidInput {
                    source: box_error(e),
                }
            })?;
        }
        PatchField::Clear => {
            product.clear_availability().map_err(|e| {
                CanonicalProductListingWriteError::InvalidInput {
                    source: box_error(e),
                }
            })?;
        }
    };
    match command.url.clone() {
        PatchField::Unchanged => {}
        PatchField::Set(v) => {
            product
                .change_url(v)
                .map_err(|e| CanonicalProductListingWriteError::InvalidInput {
                    source: box_error(e),
                })?;
        }
        PatchField::Clear => {
            return Err(CanonicalProductListingWriteError::InvalidInput {
                source: box_error(std::io::Error::other("listing URL cannot be cleared")),
            });
        }
    };
    match command.images.clone() {
        PatchField::Unchanged => {}
        PatchField::Set(v) => {
            product.replace_images(v).map_err(|e| {
                CanonicalProductListingWriteError::InvalidInput {
                    source: box_error(e),
                }
            })?;
        }
        PatchField::Clear => {
            product.replace_images(IndexSet::new()).map_err(|e| {
                CanonicalProductListingWriteError::InvalidInput {
                    source: box_error(e),
                }
            })?;
        }
    };
    Ok(())
}
fn apply_option<T>(field: &mut Option<T>, patch: PatchField<T>) {
    match patch {
        PatchField::Unchanged => {}
        PatchField::Set(value) => *field = Some(value),
        PatchField::Clear => *field = None,
    }
}
fn patch_value<T>(patch: PatchField<T>) -> Option<T> {
    match patch {
        PatchField::Set(value) => Some(value),
        PatchField::Unchanged | PatchField::Clear => None,
    }
}
fn map_repository_error(error: ProductListingRepositoryError) -> CanonicalProductListingWriteError {
    CanonicalProductListingWriteError::Persistence {
        source: box_error(error),
    }
}
