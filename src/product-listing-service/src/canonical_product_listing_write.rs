use crate::ports::{
    ProductListingAuctionOverrideRepository, ProductListingAuctionOverrideRepositoryFactory,
    ProductListingEventAppender, ProductListingEventAppenderFactory,
    ProductListingRawAuctionCapture, ProductListingRawAuctionContextAdmission,
    ProductListingRepository, ProductListingRepositoryError, ProductListingRepositoryFactory,
    ProductListingWriteEffects, stamp_product_listing_event,
};
use crate::product_listing_auction_patch::{
    ProductListingAuctionPatch, compose_product_listing_auction_patch,
};
use crate::product_listing_title_slug_creation::{
    ProductListingTitleSlugGenerator, RandomProductListingTitleSlugGenerator,
};
use application::error::{BoxError, box_error};
use application::patch_field::PatchField;
use auction_service::{
    AuctionWriteReceipt, EmbeddedAuctionMetadata, ResolveAuctionForListingError,
    ResolveAuctionForListingRequest,
    ports::{
        AuctionEventAppenderFactory, AuctionMetadataPolicyRepositoryFactory,
        AuctionRepositoryFactory,
    },
    resolve_auction_for_listing,
};
use domain_primitives::change_outcome::ChangeOutcome;
use domain_primitives::event_id::EventId;
use indexmap::IndexSet;
use listing_source_core::ListingSourceId;
use localization::{Language, Localized};
use money::Price;
use product_listing_core::description::Description;
use product_listing_core::listing_availability::ListingAvailability;
use product_listing_core::product_listing::{
    NewProductListing, ProductListing, ProductListingAuction, ProductListingPricing,
};
use product_listing_core::product_listing_id::ProductListingId;
use product_listing_core::product_listing_image::ProductListingImage;
use product_listing_core::product_listing_price::ProductListingPrice;
use product_listing_core::source_listing_id::SourceListingId;
use product_listing_core::title::Title;
use time::OffsetDateTime;
use url::Url;

/// Internal canonical write intent for caller-owned transactions.
///
/// It is not an inbound use case. `product-service` uses it to retain ProductListing
/// aggregate behavior and event semantics while owning the surrounding raw-progress transaction.
#[derive(Debug, Clone, PartialEq)]
pub struct CanonicalProductListingUpsert {
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
    /// Outer auction-context patch. `CLEAR` is non-destructive for existing listings;
    /// explicit correction owns retraction in a later iteration.
    pub auction: PatchField<ProductListingAuctionPatch>,
    pub auction_metadata: EmbeddedAuctionMetadata,
    /// Present only for immutable raw normalization. Direct partner writes do not use raw fences.
    pub raw_auction_capture: Option<ProductListingRawAuctionCapture>,
    /// Raw ingestion may preserve unrelated facts when an asserted membership conflicts. Typed
    /// partner writes remain atomic and leave this disabled.
    pub isolate_raw_auction_membership_conflict: bool,
}

/// Canonical raw-ingestion intent that cannot mutate Auction facts.
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
    /// The listing's override policy preserved its existing context; unrelated facts may still
    /// have been written in the same canonical transaction.
    pub auction_context_override_preserved: bool,
    /// Transactional evidence of an accepted reliable Auction reference, including no-op and
    /// metadata-only acceptance.
    pub auction_acceptance: Option<AuctionWriteReceipt>,
    /// A raw A-to-B membership assertion was isolated; its Auction group was not applied.
    pub auction_membership_conflict_preserved: bool,
    /// A raw timing assertion was invalid after composition with current context and was omitted.
    pub auction_timing_preserved: bool,
}

struct ResolvedAuctionContext {
    auction: Option<ProductListingAuction>,
    acceptance: Option<AuctionWriteReceipt>,
    membership_conflict_preserved: bool,
    timing_preserved: bool,
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
    #[error("auction membership change requires an explicit correction")]
    MembershipChangeRequiresCorrection,
    #[error("canonical auction resolution failed")]
    AuctionResolution {
        #[source]
        source: BoxError,
    },
    #[error("canonical product listing event append failed")]
    EventAppend {
        #[source]
        source: BoxError,
    },
}

pub struct CanonicalProductListingWriterDependencies<'a, R, E, AR, AE, AP, AO> {
    pub products: &'a R,
    pub events: &'a E,
    pub auctions: &'a AR,
    pub auction_events: &'a AE,
    pub auction_policies: &'a AP,
    pub auction_overrides: &'a AO,
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
            Some(product_listing_id) => products
                .in_transaction(tx)
                .find_by_id(product_listing_id)
                .await
                .map_err(map_repository_error)?
                .ok_or(CanonicalProductListingWriteError::BoundProductListingNotFound)?,
            None => {
                let key = product_listing_core::product_listing_id::ProductListingKey::new(
                    command.listing_source_id,
                    command.source_listing_id.clone(),
                );
                let Some(existing) = products
                    .in_transaction(tx)
                    .find_by_key(&key)
                    .await
                    .map_err(map_repository_error)?
                else {
                    return Self::create_non_auction(tx, products, events, command).await;
                };
                existing
            }
        };

        let expected_version = existing.version;
        let mut product = existing.value;
        if product.listing_source_id() != command.listing_source_id
            || product.source_listing_id() != &command.source_listing_id
        {
            return Err(CanonicalProductListingWriteError::BoundProductListingIdentityMismatch);
        }
        product
            .restore()
            .map_err(|error| CanonicalProductListingWriteError::InvalidInput {
                source: box_error(error),
            })?;
        apply_non_auction_update(&mut product, &command)?;
        let event = product.take_pending_event_payload().map(|payload| {
            stamp_product_listing_event(product.id(), OffsetDateTime::now_utc(), payload)
        });
        let event_id = event.as_ref().map(|event| event.event_id);
        if let Some(event) = event {
            let effects = ProductListingWriteEffects::from(&event.payload);
            products
                .in_transaction(tx)
                .update(&product, expected_version, event.event_id, effects)
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
            outcome: if event_id.is_some() {
                ChangeOutcome::Changed
            } else {
                ChangeOutcome::Unchanged
            },
            auction_context_override_preserved: false,
            auction_acceptance: None,
            auction_membership_conflict_preserved: false,
            auction_timing_preserved: false,
        })
    }

    async fn create_non_auction<Tx, R, E>(
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
        let slug_title = title
            .as_ref()
            .map_or("listing", |value| value.payload.as_ref());
        let title_slug_id = RandomProductListingTitleSlugGenerator
            .generate(slug_title)
            .map_err(|error| CanonicalProductListingWriteError::InvalidInput {
                source: box_error(error),
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
        .map_err(|error| CanonicalProductListingWriteError::InvalidInput {
            source: box_error(error),
        })?;
        let event = product
            .take_pending_event_payload()
            .map(|payload| {
                stamp_product_listing_event(product.id(), OffsetDateTime::now_utc(), payload)
            })
            .ok_or_else(|| CanonicalProductListingWriteError::InvalidInput {
                source: box_error(std::io::Error::other(
                    "new listing did not produce discovery event",
                )),
            })?;
        products
            .in_transaction(tx)
            .insert(&product, event.event_id)
            .await
            .map_err(map_repository_error)?;
        events
            .in_transaction(tx)
            .append(&event)
            .await
            .map_err(|error| CanonicalProductListingWriteError::EventAppend {
                source: box_error(error),
            })?;
        Ok(CanonicalProductListingWriteResult {
            product_listing_id: product.id(),
            product_listing_event_id: Some(event.event_id),
            outcome: ChangeOutcome::Changed,
            auction_context_override_preserved: false,
            auction_acceptance: None,
            auction_membership_conflict_preserved: false,
            auction_timing_preserved: false,
        })
    }

    pub async fn upsert_in_transaction<'a, Tx, R, E, AR, AE, AP, AO>(
        tx: &mut Tx,
        dependencies: CanonicalProductListingWriterDependencies<'a, R, E, AR, AE, AP, AO>,
        bound_product_listing_id: Option<ProductListingId>,
        command: CanonicalProductListingUpsert,
    ) -> Result<CanonicalProductListingWriteResult, CanonicalProductListingWriteError>
    where
        R: ProductListingRepositoryFactory<Tx>,
        E: ProductListingEventAppenderFactory<Tx>,
        AR: AuctionRepositoryFactory<Tx>,
        AE: AuctionEventAppenderFactory<Tx>,
        AP: AuctionMetadataPolicyRepositoryFactory<Tx>,
        AO: ProductListingAuctionOverrideRepositoryFactory<Tx>,
    {
        let CanonicalProductListingWriterDependencies {
            products,
            events,
            auctions,
            auction_events,
            auction_policies,
            auction_overrides,
        } = dependencies;
        let existing = match bound_product_listing_id {
            Some(product_listing_id) => products
                .in_transaction(tx)
                .find_by_id(product_listing_id)
                .await
                .map_err(map_repository_error)?
                .ok_or(CanonicalProductListingWriteError::BoundProductListingNotFound)?,
            None => {
                let key = product_listing_core::product_listing_id::ProductListingKey::new(
                    command.listing_source_id,
                    command.source_listing_id.clone(),
                );
                let existing = products
                    .in_transaction(tx)
                    .find_by_key(&key)
                    .await
                    .map_err(map_repository_error)?;
                let Some(existing) = existing else {
                    return Self::create(
                        tx,
                        products,
                        events,
                        auctions,
                        auction_events,
                        auction_policies,
                        auction_overrides,
                        command,
                    )
                    .await;
                };
                existing
            }
        };

        let expected_version = existing.version;
        let mut product = existing.value;
        if product.listing_source_id() != command.listing_source_id
            || product.source_listing_id() != &command.source_listing_id
        {
            return Err(CanonicalProductListingWriteError::BoundProductListingIdentityMismatch);
        }
        product
            .restore()
            .map_err(|error| CanonicalProductListingWriteError::InvalidInput {
                source: box_error(error),
            })?;
        let (command, auction_context_override_preserved) =
            preserve_overridden_auction_context(tx, auction_overrides, product.id(), command)
                .await?;
        let auction = resolve_auction_context(
            tx,
            auctions,
            auction_events,
            auction_policies,
            product.listing_source_id(),
            product.auction(),
            &command,
        )
        .await?;
        apply_update(&mut product, &command, auction.auction)?;
        let event = product.take_pending_event_payload().map(|payload| {
            stamp_product_listing_event(product.id(), OffsetDateTime::now_utc(), payload)
        });
        let event_id = event.as_ref().map(|event| event.event_id);
        if let Some(event) = event {
            let effects = ProductListingWriteEffects::from(&event.payload);
            products
                .in_transaction(tx)
                .update(&product, expected_version, event.event_id, effects)
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
            outcome: if event_id.is_some() {
                ChangeOutcome::Changed
            } else {
                ChangeOutcome::Unchanged
            },
            auction_context_override_preserved,
            auction_acceptance: auction.acceptance,
            auction_membership_conflict_preserved: auction.membership_conflict_preserved,
            auction_timing_preserved: auction.timing_preserved,
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
        let loaded = products
            .in_transaction(tx)
            .find_by_id(product_listing_id)
            .await
            .map_err(map_repository_error)?
            .ok_or(CanonicalProductListingWriteError::BoundProductListingNotFound)?;
        let expected_version = loaded.version;
        let mut product = loaded.value;
        product.withdraw().map_err(invalid_input)?;
        let event = product.take_pending_event_payload().map(|payload| {
            stamp_product_listing_event(product.id(), OffsetDateTime::now_utc(), payload)
        });
        let event_id = event.as_ref().map(|event| event.event_id);
        if let Some(event) = event {
            let effects = ProductListingWriteEffects::from(&event.payload);
            products
                .in_transaction(tx)
                .update(&product, expected_version, event.event_id, effects)
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
            product_listing_id,
            product_listing_event_id: event_id,
            outcome: if event_id.is_some() {
                ChangeOutcome::Changed
            } else {
                ChangeOutcome::Unchanged
            },
            auction_context_override_preserved: false,
            auction_acceptance: None,
            auction_membership_conflict_preserved: false,
            auction_timing_preserved: false,
        })
    }

    #[allow(clippy::too_many_arguments)]
    async fn create<Tx, R, E, AR, AE, AP, AO>(
        tx: &mut Tx,
        products: &R,
        events: &E,
        auctions: &AR,
        auction_events: &AE,
        auction_policies: &AP,
        _auction_overrides: &AO,
        command: CanonicalProductListingUpsert,
    ) -> Result<CanonicalProductListingWriteResult, CanonicalProductListingWriteError>
    where
        R: ProductListingRepositoryFactory<Tx>,
        E: ProductListingEventAppenderFactory<Tx>,
        AR: AuctionRepositoryFactory<Tx>,
        AE: AuctionEventAppenderFactory<Tx>,
        AP: AuctionMetadataPolicyRepositoryFactory<Tx>,
        AO: ProductListingAuctionOverrideRepositoryFactory<Tx>,
    {
        let url = match &command.url {
            PatchField::Set(url) => url.clone(),
            PatchField::Unchanged | PatchField::Clear => {
                return Err(CanonicalProductListingWriteError::InvalidInput {
                    source: box_error(std::io::Error::other("new raw listing requires URL")),
                });
            }
        };
        let auction = resolve_auction_context(
            tx,
            auctions,
            auction_events,
            auction_policies,
            command.listing_source_id,
            None,
            &command,
        )
        .await?;
        let title = patch_value(command.title);
        let slug_title = title
            .as_ref()
            .map_or("listing", |value| value.payload.as_ref());
        let title_slug_id = RandomProductListingTitleSlugGenerator
            .generate(slug_title)
            .map_err(|error| CanonicalProductListingWriteError::InvalidInput {
                source: box_error(error),
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
            auction: auction.auction,
        })
        .map_err(|error| CanonicalProductListingWriteError::InvalidInput {
            source: box_error(error),
        })?;
        let event = product
            .take_pending_event_payload()
            .map(|payload| {
                stamp_product_listing_event(product.id(), OffsetDateTime::now_utc(), payload)
            })
            .ok_or_else(|| CanonicalProductListingWriteError::InvalidInput {
                source: box_error(std::io::Error::other(
                    "new listing did not produce discovery event",
                )),
            })?;
        products
            .in_transaction(tx)
            .insert(&product, event.event_id)
            .await
            .map_err(map_repository_error)?;
        events
            .in_transaction(tx)
            .append(&event)
            .await
            .map_err(|error| CanonicalProductListingWriteError::EventAppend {
                source: box_error(error),
            })?;
        Ok(CanonicalProductListingWriteResult {
            product_listing_id: product.id(),
            product_listing_event_id: Some(event.event_id),
            outcome: ChangeOutcome::Changed,
            auction_context_override_preserved: false,
            auction_acceptance: auction.acceptance,
            auction_membership_conflict_preserved: auction.membership_conflict_preserved,
            auction_timing_preserved: auction.timing_preserved,
        })
    }
}

async fn preserve_overridden_auction_context<Tx, AO>(
    tx: &mut Tx,
    overrides: &AO,
    product_listing_id: ProductListingId,
    mut command: CanonicalProductListingUpsert,
) -> Result<(CanonicalProductListingUpsert, bool), CanonicalProductListingWriteError>
where
    AO: ProductListingAuctionOverrideRepositoryFactory<Tx>,
{
    if matches!(command.auction, PatchField::Unchanged) {
        return Ok((command, false));
    }
    overrides
        .in_transaction(tx)
        .lock(product_listing_id)
        .await
        .map_err(|error| CanonicalProductListingWriteError::Persistence {
            source: box_error(error),
        })?;
    let preserve = if let Some(raw_capture) = command.raw_auction_capture {
        matches!(
            overrides
                .in_transaction(tx)
                .admit_raw_auction_context(product_listing_id, raw_capture)
                .await
                .map_err(|error| CanonicalProductListingWriteError::Persistence {
                    source: box_error(error),
                })?,
            ProductListingRawAuctionContextAdmission::Preserve
        )
    } else {
        overrides
            .in_transaction(tx)
            .find(product_listing_id)
            .await
            .map_err(|error| CanonicalProductListingWriteError::Persistence {
                source: box_error(error),
            })?
            .is_some_and(|policy| policy.active)
    };
    if preserve {
        command.auction = PatchField::Unchanged;
        command.auction_metadata = EmbeddedAuctionMetadata::default();
    }
    Ok((command, preserve))
}

async fn resolve_auction_context<Tx, AR, AE, AP>(
    tx: &mut Tx,
    auctions: &AR,
    auction_events: &AE,
    auction_policies: &AP,
    listing_source_id: ListingSourceId,
    existing: Option<&ProductListingAuction>,
    command: &CanonicalProductListingUpsert,
) -> Result<ResolvedAuctionContext, CanonicalProductListingWriteError>
where
    AR: AuctionRepositoryFactory<Tx>,
    AE: AuctionEventAppenderFactory<Tx>,
    AP: AuctionMetadataPolicyRepositoryFactory<Tx>,
{
    let PatchField::Set(patch) = &command.auction else {
        return Ok(ResolvedAuctionContext {
            auction: None,
            acceptance: None,
            membership_conflict_preserved: false,
            timing_preserved: false,
        });
    };
    let current_membership = existing.and_then(ProductListingAuction::membership);
    let mut acceptance = None;
    let membership = match patch.source_auction_id.clone() {
        PatchField::Set(source_auction_id) => {
            let receipt = resolve_auction_for_listing(
                tx,
                auctions,
                auction_events,
                auction_policies,
                ResolveAuctionForListingRequest {
                    listing_source_id,
                    source_auction_id,
                    current_membership: current_membership.map(|value| value.auction_id()),
                    metadata: command.auction_metadata.clone(),
                },
            )
            .await;
            match receipt {
                Ok(receipt) => {
                    let membership = product_listing_core::product_listing::AuctionMembership::new(
                        receipt.auction_id,
                    );
                    acceptance = Some(receipt);
                    Some(membership)
                }
                Err(ResolveAuctionForListingError::MembershipChangeRequiresCorrection)
                    if command.isolate_raw_auction_membership_conflict =>
                {
                    return Ok(ResolvedAuctionContext {
                        auction: None,
                        acceptance: None,
                        membership_conflict_preserved: true,
                        timing_preserved: false,
                    });
                }
                Err(ResolveAuctionForListingError::MembershipChangeRequiresCorrection) => {
                    return Err(
                        CanonicalProductListingWriteError::MembershipChangeRequiresCorrection,
                    );
                }
                Err(source) => {
                    return Err(CanonicalProductListingWriteError::AuctionResolution {
                        source: box_error(source),
                    });
                }
            }
        }
        PatchField::Clear | PatchField::Unchanged => current_membership,
    };
    match compose_product_listing_auction_patch(existing, membership, patch) {
        Ok(auction) => Ok(ResolvedAuctionContext {
            auction: Some(auction),
            acceptance,
            membership_conflict_preserved: false,
            timing_preserved: false,
        }),
        Err(_) if command.isolate_raw_auction_membership_conflict => {
            let patch_without_timing = ProductListingAuctionPatch {
                source_auction_id: patch.source_auction_id.clone(),
                lot_number: patch.lot_number.clone(),
                catalogue_position: patch.catalogue_position.clone(),
                ..Default::default()
            };
            compose_product_listing_auction_patch(existing, membership, &patch_without_timing)
                .map(|auction| ResolvedAuctionContext {
                    auction: Some(auction),
                    acceptance,
                    membership_conflict_preserved: false,
                    timing_preserved: true,
                })
                .map_err(invalid_input)
        }
        Err(error) => Err(invalid_input(error)),
    }
}

fn patch_value<T>(patch: PatchField<T>) -> Option<T> {
    match patch {
        PatchField::Set(value) => Some(value),
        PatchField::Unchanged | PatchField::Clear => None,
    }
}

fn apply_non_auction_update(
    product: &mut ProductListing,
    command: &CanonicalProductListingNonAuctionUpsert,
) -> Result<(), CanonicalProductListingWriteError> {
    apply_update(
        product,
        &CanonicalProductListingUpsert {
            listing_source_id: command.listing_source_id,
            source_listing_id: command.source_listing_id.clone(),
            title: command.title.clone(),
            description: command.description.clone(),
            price: command.price.clone(),
            price_estimate_min: command.price_estimate_min.clone(),
            price_estimate_max: command.price_estimate_max.clone(),
            availability: command.availability.clone(),
            url: command.url.clone(),
            images: command.images.clone(),
            auction: PatchField::Unchanged,
            auction_metadata: EmbeddedAuctionMetadata::default(),
            raw_auction_capture: None,
            isolate_raw_auction_membership_conflict: false,
        },
        None,
    )
}

fn apply_update(
    product: &mut ProductListing,
    command: &CanonicalProductListingUpsert,
    auction: Option<ProductListingAuction>,
) -> Result<(), CanonicalProductListingWriteError> {
    let mut pricing = product.pricing();
    apply_option_patch(&mut pricing.price, command.price.clone());
    apply_option_patch(
        &mut pricing.price_estimate_min,
        command.price_estimate_min.clone(),
    );
    apply_option_patch(
        &mut pricing.price_estimate_max,
        command.price_estimate_max.clone(),
    );
    product.replace_pricing(pricing).map_err(invalid_input)?;

    match command.availability {
        PatchField::Set(availability) => {
            product
                .set_availability(availability)
                .map_err(invalid_input)?;
        }
        PatchField::Clear => {
            product.clear_availability().map_err(invalid_input)?;
        }
        PatchField::Unchanged => {}
    }
    if let PatchField::Set(url) = &command.url {
        product.change_url(url.clone()).map_err(invalid_input)?;
    }
    match &command.images {
        PatchField::Set(images) => {
            product
                .replace_images(images.clone())
                .map_err(invalid_input)?;
        }
        PatchField::Clear => {
            product
                .replace_images(IndexSet::new())
                .map_err(invalid_input)?;
        }
        PatchField::Unchanged => {}
    };
    if let Some(auction) = auction {
        product
            .replace_auction(Some(auction))
            .map_err(invalid_input)?;
    }
    Ok(())
}

fn apply_option_patch<T>(target: &mut Option<T>, patch: PatchField<T>) {
    match patch {
        PatchField::Set(value) => *target = Some(value),
        PatchField::Clear => *target = None,
        PatchField::Unchanged => {}
    }
}

fn invalid_input(
    error: impl std::error::Error + Send + Sync + 'static,
) -> CanonicalProductListingWriteError {
    CanonicalProductListingWriteError::InvalidInput {
        source: box_error(error),
    }
}

fn map_repository_error(error: ProductListingRepositoryError) -> CanonicalProductListingWriteError {
    CanonicalProductListingWriteError::Persistence {
        source: box_error(error),
    }
}
