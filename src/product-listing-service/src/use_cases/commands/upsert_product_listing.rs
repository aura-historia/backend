use crate::ports::{
    PartnerProductListingAuthorizationError, PartnerProductListingAuthorizer,
    PartnerProductListingAuthorizerFactory, ProductListingEventStore,
    ProductListingEventStoreError, ProductListingEventStoreFactory, ProductListingRepository,
    ProductListingRepositoryError, ProductListingRepositoryFactory, stamp_product_listing_events,
};
use crate::product_listing_title_slug_creation::{
    ProductListingTitleSlugGenerator, RandomProductListingTitleSlugGenerator,
    TitleSlugCollisionRetry, title_slug_collision_retry,
};
use crate::use_cases::{CreateProductListingResult, UpdateProductListingResult};
use application::error::{BoxError, box_error};
use application::operation_context::{
    CredentialCapability, OperationAuthorizationError, OperationContext, Principal,
};
use application::patch_field::PatchField;
use application::transaction::{Transaction, UnitOfWork};

use indexmap::IndexSet;
use listing_source_core::ListingSourceId;
use localization::{Language, Localized};
use money::Price;
use product_listing_core::description::Description;
use product_listing_core::listing_availability::ListingAvailability;
use product_listing_core::product_listing::{
    ChangeListingAvailabilityError, ChangeProductListingError, NewProductListing, ProductListing,
    ProductListingAuction, ProductListingPricing, RehydrateProductListingError,
};
use product_listing_core::product_listing_id::{ProductListingId, ProductListingKey};
use product_listing_core::product_listing_image::ProductListingImage;
use product_listing_core::product_listing_slug_id::ProductListingSlugId;
use product_listing_core::source_listing_id::SourceListingId;
use product_listing_core::title::Title;
use url::Url;
use user_core::user_id::UserId;

const MISSING_PRODUCT_URL: &str = "https://not-provided.invalid";
#[derive(Debug, Clone, PartialEq)]
pub struct UpsertProductListingCommand {
    pub listing_source_id: ListingSourceId,
    pub source_listing_id: SourceListingId,
    pub title: Option<Localized<Language, Title>>,
    pub description: Option<Localized<Language, Description>>,
    pub price: PatchField<Price>,
    pub price_estimate_min: PatchField<Price>,
    pub price_estimate_max: PatchField<Price>,
    pub availability: PatchField<ListingAvailability>,
    pub url: Option<Url>,
    pub images: PatchField<IndexSet<ProductListingImage>>,
    pub auction_start: PatchField<time::OffsetDateTime>,
    pub auction_end: PatchField<time::OffsetDateTime>,
}
#[derive(Debug, Clone, PartialEq)]
pub enum UpsertProductListingResult {
    Created(CreateProductListingResult),
    Updated(UpdateProductListingResult),
}
#[derive(Debug, thiserror::Error)]
pub enum UpsertProductListingError {
    #[error("authenticated actor required to upsert product listing")]
    AuthenticatedActorRequired,
    #[error("operation not permitted")]
    Forbidden,
    #[error("listing source not found")]
    ListingSourceNotFound,
    #[error("partner product listing authorization is temporarily unavailable")]
    PartnerAuthorizationTemporarilyUnavailable {
        #[source]
        source: BoxError,
    },
    #[error("partner product listing authorization failed internally")]
    PartnerAuthorizationInternal {
        #[source]
        source: BoxError,
    },
    #[error("product listing is withdrawn")]
    ListingWithdrawn,
    #[error("product listing is invalid")]
    InvalidProductListing {
        #[source]
        source: BoxError,
    },
    #[error("product listing title slug generation was exhausted")]
    ProductListingTitleSlugGenerationExhausted,
    #[error("product listing persistence failed")]
    PersistenceFailed,
    #[error("product listing event storage failed")]
    EventStoreFailed,
    #[error("failed to begin upsert product listing transaction")]
    BeginTransactionFailed,
    #[error("failed to commit upsert product listing transaction")]
    CommitTransactionFailed,
}
#[async_trait::async_trait]
pub trait UpsertProductListingUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        command: UpsertProductListingCommand,
    ) -> Result<UpsertProductListingResult, UpsertProductListingError>;
}
pub struct UpsertProductListingHandler<U, R, E, A, G = RandomProductListingTitleSlugGenerator> {
    unit_of_work: U,
    products: R,
    events: E,
    authorizer: A,
    title_slug_generator: G,
}

enum UpsertAttemptError {
    SourceListingInsertRace,
    TitleSlugCollision,
    Failed(UpsertProductListingError),
}

impl From<UpsertProductListingError> for UpsertAttemptError {
    fn from(error: UpsertProductListingError) -> Self {
        Self::Failed(error)
    }
}

impl From<ProductListingRepositoryError> for UpsertAttemptError {
    fn from(error: ProductListingRepositoryError) -> Self {
        Self::Failed(error.into())
    }
}

impl From<ProductListingEventStoreError> for UpsertAttemptError {
    fn from(error: ProductListingEventStoreError) -> Self {
        Self::Failed(error.into())
    }
}

impl From<PartnerProductListingAuthorizationError> for UpsertAttemptError {
    fn from(error: PartnerProductListingAuthorizationError) -> Self {
        Self::Failed(error.into())
    }
}

impl From<ChangeListingAvailabilityError> for UpsertAttemptError {
    fn from(error: ChangeListingAvailabilityError) -> Self {
        Self::Failed(error.into())
    }
}

impl From<ChangeProductListingError> for UpsertAttemptError {
    fn from(error: ChangeProductListingError) -> Self {
        Self::Failed(error.into())
    }
}

impl From<RehydrateProductListingError> for UpsertAttemptError {
    fn from(error: RehydrateProductListingError) -> Self {
        Self::Failed(error.into())
    }
}
impl<U, R, E, A> UpsertProductListingHandler<U, R, E, A, RandomProductListingTitleSlugGenerator> {
    pub fn new(unit_of_work: U, products: R, events: E, authorizer: A) -> Self {
        Self::with_title_slug_generator(
            unit_of_work,
            products,
            events,
            authorizer,
            RandomProductListingTitleSlugGenerator,
        )
    }
}
impl<U, R, E, A, G> UpsertProductListingHandler<U, R, E, A, G> {
    pub fn with_title_slug_generator(
        unit_of_work: U,
        products: R,
        events: E,
        authorizer: A,
        title_slug_generator: G,
    ) -> Self {
        Self {
            unit_of_work,
            products,
            events,
            authorizer,
            title_slug_generator,
        }
    }
}
impl<U, R, E, A, G> UpsertProductListingHandler<U, R, E, A, G>
where
    U: UnitOfWork,
    R: ProductListingRepositoryFactory<U::Tx>,
    E: ProductListingEventStoreFactory<U::Tx>,
    A: PartnerProductListingAuthorizerFactory<U::Tx>,
    G: ProductListingTitleSlugGenerator,
{
    async fn persist(
        &self,
        tx: &mut U::Tx,
        context: &OperationContext,
        command: UpsertProductListingCommand,
        new_product_listing_id: ProductListingId,
        title_slug_id: ProductListingSlugId,
    ) -> Result<UpsertProductListingResult, UpsertAttemptError> {
        if let Some(actor_id) = partner_actor(&context.principal) {
            self.authorizer
                .in_transaction(tx)
                .authorize(actor_id, command.listing_source_id)
                .await?;
        }
        let key =
            ProductListingKey::new(command.listing_source_id, command.source_listing_id.clone());
        let existing = self.products.in_transaction(tx).find_by_key(&key).await?;
        match existing {
            Some(loaded) => {
                let expected_event_id = loaded.version;
                let mut product = loaded.value;
                product.restore();
                apply_update(&mut product, &command)?;
                let events = stamp_product_listing_events(
                    product.id(),
                    time::OffsetDateTime::now_utc(),
                    product.take_pending_event_payloads(),
                );
                let event_id = events.last().map(|event| event.event_id);
                if let Some(new_event_id) = event_id {
                    product = self
                        .products
                        .in_transaction(tx)
                        .update(&product, expected_event_id, new_event_id)
                        .await?
                        .value;
                    for event in &events {
                        self.events.in_transaction(tx).append(event).await?;
                    }
                }
                Ok(UpsertProductListingResult::Updated(
                    UpdateProductListingResult {
                        product_listing_id: product.id(),
                        event_id,
                    },
                ))
            }
            None => {
                let mut product = ProductListing::create(
                    command.into_new_product(new_product_listing_id, title_slug_id)?,
                )?;
                let events = stamp_product_listing_events(
                    product.id(),
                    time::OffsetDateTime::now_utc(),
                    product.take_pending_event_payloads(),
                );
                let event_id = events.last().map(|event| event.event_id).ok_or_else(|| {
                    UpsertProductListingError::InvalidProductListing {
                        source: box_error(std::io::Error::other("created listing has no event")),
                    }
                })?;
                let persisted = self
                    .products
                    .in_transaction(tx)
                    .insert(&product, event_id)
                    .await
                    .map_err(|error| match error {
                        ProductListingRepositoryError::SourceListingAlreadyExists => {
                            UpsertAttemptError::SourceListingInsertRace
                        }
                        ProductListingRepositoryError::ProductListingTitleSlugAlreadyExists => {
                            UpsertAttemptError::TitleSlugCollision
                        }
                        error => UpsertAttemptError::Failed(error.into()),
                    })?;
                for event in &events {
                    self.events.in_transaction(tx).append(event).await?;
                }
                Ok(UpsertProductListingResult::Created(
                    CreateProductListingResult {
                        product_listing_id: persisted.value.id(),
                        product_listing_title_slug_id: persisted.value.title_slug_id().clone(),
                        event_id,
                    },
                ))
            }
        }
    }

    async fn execute_attempt(
        &self,
        context: &OperationContext,
        command: UpsertProductListingCommand,
        new_product_listing_id: ProductListingId,
        title_slug_id: ProductListingSlugId,
    ) -> Result<UpsertProductListingResult, UpsertAttemptError> {
        context
            .require()
            .credential_capability(CredentialCapability::ProductListingsWrite)
            .authorize::<UpsertProductListingError>()?;
        let mut tx = self
            .unit_of_work
            .begin()
            .await
            .map_err(|_| UpsertProductListingError::BeginTransactionFailed)?;
        let result = self
            .persist(
                &mut tx,
                context,
                command,
                new_product_listing_id,
                title_slug_id,
            )
            .await?;
        tx.commit()
            .await
            .map_err(|_| UpsertProductListingError::CommitTransactionFailed)?;
        Ok(result)
    }
}
#[async_trait::async_trait]
impl<U, R, E, A, G> UpsertProductListingUseCase for UpsertProductListingHandler<U, R, E, A, G>
where
    U: UnitOfWork,
    R: ProductListingRepositoryFactory<U::Tx>,
    E: ProductListingEventStoreFactory<U::Tx>,
    A: PartnerProductListingAuthorizerFactory<U::Tx>,
    G: ProductListingTitleSlugGenerator,
{
    #[tracing::instrument(name = "upsert_product_listing", skip_all, fields(listing_source_id = %command.listing_source_id, source_listing_id = %command.source_listing_id, principal_type = context.principal.kind(), actor_id = tracing::field::Empty, request_id = %context.request_id, correlation_id = %context.correlation_id))]
    async fn execute(
        &self,
        context: &OperationContext,
        command: UpsertProductListingCommand,
    ) -> Result<UpsertProductListingResult, UpsertProductListingError> {
        tracing::Span::current().record(
            "actor_id",
            tracing::field::display(context.principal.label()),
        );
        let new_product_listing_id = ProductListingId::new();
        let mut source_listing_races = 0;
        let mut title_slug_attempts = 0;
        let result = loop {
            let title_slug_id = self
                .title_slug_generator
                .generate(
                    command
                        .title
                        .as_ref()
                        .map_or("", |title| title.payload.as_ref()),
                )
                .map_err(|_| UpsertProductListingError::InvalidProductListing {
                    source: box_error(std::io::Error::other("invalid generated title slug")),
                })?;
            match self
                .execute_attempt(
                    context,
                    command.clone(),
                    new_product_listing_id,
                    title_slug_id,
                )
                .await
            {
                Ok(result) => break result,
                Err(UpsertAttemptError::SourceListingInsertRace) => {
                    source_listing_races += 1;
                    if source_listing_races > 1 {
                        return Err(UpsertProductListingError::PersistenceFailed);
                    }
                }
                Err(UpsertAttemptError::TitleSlugCollision) => {
                    title_slug_attempts += 1;
                    if title_slug_collision_retry(title_slug_attempts, true)
                        == TitleSlugCollisionRetry::Exhausted
                    {
                        return Err(
                            UpsertProductListingError::ProductListingTitleSlugGenerationExhausted,
                        );
                    }
                    tracing::warn!(
                        product_listing_id = %new_product_listing_id,
                        attempt = title_slug_attempts,
                        constraint_name = "product_listings_title_slug_unique",
                        "product listing title slug collision; regenerating"
                    );
                }
                Err(UpsertAttemptError::Failed(error)) => return Err(error),
            }
        };
        let product_listing_id = match &result {
            UpsertProductListingResult::Created(value) => value.product_listing_id,
            UpsertProductListingResult::Updated(value) => value.product_listing_id,
        };
        tracing::info!(event = "product_listing.upserted", actor_type = context.principal.kind(), actor_id = %context.principal.label(), product_listing_id = %product_listing_id, outcome = "success");
        Ok(result)
    }
}
impl UpsertProductListingCommand {
    fn into_new_product(
        self,
        id: ProductListingId,
        title_slug_id: ProductListingSlugId,
    ) -> Result<NewProductListing, UpsertProductListingError> {
        let url = match self.url {
            Some(url) => url,
            None => Url::parse(MISSING_PRODUCT_URL).map_err(|error| {
                UpsertProductListingError::InvalidProductListing {
                    source: box_error(error),
                }
            })?,
        };
        Ok(NewProductListing {
            id,
            title_slug_id,
            listing_source_id: self.listing_source_id,
            source_listing_id: self.source_listing_id,
            title: self.title,
            description: self.description,
            pricing: ProductListingPricing {
                price: match self.price {
                    PatchField::Set(price) => Some(price),
                    PatchField::Unchanged | PatchField::Clear => None,
                },
                price_estimate_min: optional_patch_into_value(self.price_estimate_min),
                price_estimate_max: optional_patch_into_value(self.price_estimate_max),
            },
            availability: match self.availability {
                PatchField::Unchanged | PatchField::Clear => None,
                PatchField::Set(availability) => Some(availability),
            },
            url,
            images: collection_patch_into_value(self.images),
            auction: ProductListingAuction {
                start: optional_patch_into_value(self.auction_start),
                end: optional_patch_into_value(self.auction_end),
            },
        })
    }
}
fn apply_update(
    product: &mut ProductListing,
    command: &UpsertProductListingCommand,
) -> Result<(), UpsertProductListingError> {
    let mut pricing = product.pricing();
    apply_optional_patch(&mut pricing.price, command.price.clone());
    apply_optional_patch(
        &mut pricing.price_estimate_min,
        command.price_estimate_min.clone(),
    );
    apply_optional_patch(
        &mut pricing.price_estimate_max,
        command.price_estimate_max.clone(),
    );
    product.replace_pricing(pricing)?;
    match command.availability {
        PatchField::Unchanged => {}
        PatchField::Set(availability) => {
            product.set_availability(availability)?;
        }
        PatchField::Clear => {
            product.clear_availability()?;
        }
    }
    if let Some(url) = &command.url {
        product.change_url(url.clone())?;
    }
    match &command.images {
        PatchField::Unchanged => {}
        PatchField::Set(images) => {
            product.replace_images(images.clone())?;
        }
        PatchField::Clear => {
            product.replace_images(IndexSet::new())?;
        }
    }
    let mut auction = product.auction();
    apply_optional_patch(&mut auction.start, command.auction_start.clone());
    apply_optional_patch(&mut auction.end, command.auction_end.clone());
    product.replace_auction(auction)?;
    Ok(())
}

fn apply_optional_patch<T>(field: &mut Option<T>, patch: PatchField<T>) {
    match patch {
        PatchField::Unchanged => {}
        PatchField::Set(value) => *field = Some(value),
        PatchField::Clear => *field = None,
    }
}

fn optional_patch_into_value<T>(patch: PatchField<T>) -> Option<T> {
    match patch {
        PatchField::Set(value) => Some(value),
        PatchField::Unchanged | PatchField::Clear => None,
    }
}

fn collection_patch_into_value<T>(patch: PatchField<IndexSet<T>>) -> IndexSet<T>
where
    T: Eq + std::hash::Hash,
{
    match patch {
        PatchField::Set(value) => value,
        PatchField::Unchanged | PatchField::Clear => IndexSet::new(),
    }
}

fn partner_actor(principal: &Principal) -> Option<UserId> {
    match principal {
        Principal::User(id) | Principal::DelegatedUser { user_id: id, .. } => Some(*id),
        Principal::Anonymous | Principal::Service(_) | Principal::System => None,
    }
}
impl From<OperationAuthorizationError> for UpsertProductListingError {
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
impl From<PartnerProductListingAuthorizationError> for UpsertProductListingError {
    fn from(error: PartnerProductListingAuthorizationError) -> Self {
        match error {
            PartnerProductListingAuthorizationError::ListingSourceNotFound => {
                Self::ListingSourceNotFound
            }
            PartnerProductListingAuthorizationError::Forbidden => Self::Forbidden,
            PartnerProductListingAuthorizationError::TemporarilyUnavailable { source } => {
                Self::PartnerAuthorizationTemporarilyUnavailable { source }
            }
            PartnerProductListingAuthorizationError::Internal { source } => {
                Self::PartnerAuthorizationInternal { source }
            }
        }
    }
}
impl From<ChangeListingAvailabilityError> for UpsertProductListingError {
    fn from(_: ChangeListingAvailabilityError) -> Self {
        Self::ListingWithdrawn
    }
}
impl From<ChangeProductListingError> for UpsertProductListingError {
    fn from(error: ChangeProductListingError) -> Self {
        match error {
            ChangeProductListingError::ListingWithdrawn => Self::ListingWithdrawn,
            ChangeProductListingError::AuctionStartAfterEnd => Self::InvalidProductListing {
                source: box_error(error),
            },
        }
    }
}
impl From<RehydrateProductListingError> for UpsertProductListingError {
    fn from(error: RehydrateProductListingError) -> Self {
        Self::InvalidProductListing {
            source: box_error(error),
        }
    }
}
impl From<ProductListingRepositoryError> for UpsertProductListingError {
    fn from(_: ProductListingRepositoryError) -> Self {
        Self::PersistenceFailed
    }
}
impl From<ProductListingEventStoreError> for UpsertProductListingError {
    fn from(_: ProductListingEventStoreError) -> Self {
        Self::EventStoreFailed
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use listing_source_core::ListingSourceId;
    use money::{Currency, MonetaryAmount};
    use product_listing_core::product_listing::ProductListingEventPayload;
    use product_listing_core::source_listing_id::SourceListingId;

    fn price(amount: u64) -> Price {
        Price::new(MonetaryAmount::from(amount), Currency::Eur)
    }

    fn command(price: PatchField<Price>) -> UpsertProductListingCommand {
        UpsertProductListingCommand {
            listing_source_id: ListingSourceId::new(),
            source_listing_id: SourceListingId::try_from("listing")
                .unwrap_or_else(|error| panic!("valid source listing ID: {error}")),
            title: None,
            description: None,
            price,
            price_estimate_min: PatchField::Unchanged,
            price_estimate_max: PatchField::Unchanged,
            availability: PatchField::Unchanged,
            url: None,
            images: PatchField::Unchanged,
            auction_start: PatchField::Unchanged,
            auction_end: PatchField::Unchanged,
        }
    }

    fn test_title_slug() -> ProductListingSlugId {
        ProductListingSlugId::raw("listing-a1b2c3")
            .unwrap_or_else(|error| panic!("valid product listing title slug: {error}"))
    }

    fn listing_with_price(value: Option<Price>) -> ProductListing {
        listing_with_state(
            ProductListingPricing {
                price: value,
                ..Default::default()
            },
            IndexSet::new(),
            ProductListingAuction::default(),
        )
    }

    fn listing_with_state(
        pricing: ProductListingPricing,
        images: IndexSet<ProductListingImage>,
        auction: ProductListingAuction,
    ) -> ProductListing {
        ProductListing::create(NewProductListing {
            id: ProductListingId::new(),
            title_slug_id: test_title_slug(),
            listing_source_id: ListingSourceId::new(),
            source_listing_id: SourceListingId::try_from("listing")
                .unwrap_or_else(|error| panic!("valid source listing ID: {error}")),
            title: None,
            description: None,
            pricing,
            availability: None,
            url: Url::parse("https://example.com/listing")
                .unwrap_or_else(|error| panic!("url: {error}")),
            images,
            auction,
        })
        .unwrap_or_else(|error| panic!("listing: {error}"))
    }

    #[test]
    fn should_apply_main_price_patch_for_existing_listing() {
        for (patch, expected) in [
            (PatchField::Unchanged, Some(price(100))),
            (PatchField::Set(price(120)), Some(price(120))),
            (PatchField::Clear, None),
        ] {
            let mut listing = listing_with_price(Some(price(100)));
            listing.take_pending_event_payloads();
            let changed = patch.is_changed();
            let update = command(patch);

            apply_update(&mut listing, &update).unwrap_or_else(|error| panic!("update: {error}"));

            assert_eq!(expected, listing.pricing().price);
            assert_eq!(changed, !listing.pending_event_payloads().is_empty());
        }
    }

    #[test]
    fn should_apply_estimate_patch_intent_for_existing_listing() {
        for (field, patch, expected) in [
            ("min", PatchField::Unchanged, Some(price(100))),
            ("min", PatchField::Set(price(120)), Some(price(120))),
            ("min", PatchField::Clear, None),
            ("max", PatchField::Unchanged, Some(price(200))),
            ("max", PatchField::Set(price(220)), Some(price(220))),
            ("max", PatchField::Clear, None),
        ] {
            let mut listing = listing_with_state(
                ProductListingPricing {
                    price_estimate_min: Some(price(100)),
                    price_estimate_max: Some(price(200)),
                    ..Default::default()
                },
                IndexSet::new(),
                ProductListingAuction::default(),
            );
            listing.take_pending_event_payloads();
            let mut update = command(PatchField::Unchanged);
            if field == "min" {
                update.price_estimate_min = patch;
            } else {
                update.price_estimate_max = patch;
            }

            apply_update(&mut listing, &update).unwrap_or_else(|error| panic!("update: {error}"));

            let actual = if field == "min" {
                listing.pricing().price_estimate_min
            } else {
                listing.pricing().price_estimate_max
            };
            assert_eq!(expected, actual);
        }
    }

    #[test]
    fn should_not_emit_price_event_when_clearing_absent_estimate() {
        let mut listing = listing_with_state(
            ProductListingPricing::default(),
            IndexSet::new(),
            ProductListingAuction::default(),
        );
        listing.take_pending_event_payloads();
        let mut update = command(PatchField::Unchanged);
        update.price_estimate_min = PatchField::Clear;

        apply_update(&mut listing, &update).unwrap_or_else(|error| panic!("update: {error}"));

        assert!(listing.pending_event_payloads().is_empty());
    }

    #[test]
    fn should_emit_one_price_event_with_final_pricing_for_combined_patch() {
        let old_pricing = ProductListingPricing {
            price: Some(price(100)),
            price_estimate_min: Some(price(110)),
            price_estimate_max: Some(price(120)),
        };
        let new_pricing = ProductListingPricing {
            price: Some(price(200)),
            price_estimate_min: Some(price(210)),
            price_estimate_max: Some(price(220)),
        };
        let mut listing = listing_with_state(
            old_pricing,
            IndexSet::new(),
            ProductListingAuction::default(),
        );
        listing.take_pending_event_payloads();
        let mut update = command(PatchField::Set(price(200)));
        update.price_estimate_min = PatchField::Set(price(210));
        update.price_estimate_max = PatchField::Set(price(220));

        apply_update(&mut listing, &update).unwrap_or_else(|error| panic!("update: {error}"));

        assert_eq!(listing.pricing(), new_pricing);
        assert!(matches!(
            listing.pending_event_payloads(),
            [ProductListingEventPayload::PriceChanged(change)]
                if change.old_pricing == old_pricing && change.new_pricing == new_pricing
        ));
    }

    #[test]
    fn should_apply_images_patch_intent_for_existing_listing() {
        let image = ProductListingImage::new(
            Url::parse("https://example.com/image.jpg")
                .unwrap_or_else(|error| panic!("image URL: {error}")),
        );
        for patch in [
            PatchField::Unchanged,
            PatchField::Set(IndexSet::new()),
            PatchField::Clear,
        ] {
            let mut listing = listing_with_state(
                ProductListingPricing::default(),
                IndexSet::from([image.clone()]),
                ProductListingAuction::default(),
            );
            listing.take_pending_event_payloads();
            let mut update = command(PatchField::Unchanged);
            update.images = patch.clone();

            apply_update(&mut listing, &update).unwrap_or_else(|error| panic!("update: {error}"));

            let expected = match patch {
                PatchField::Unchanged => IndexSet::from([image.clone()]),
                PatchField::Set(images) => images,
                PatchField::Clear => IndexSet::new(),
            };
            assert_eq!(listing.images(), &expected);
        }
    }

    #[test]
    fn should_apply_auction_patches_atomically() {
        let old = ProductListingAuction {
            start: Some(time::macros::datetime!(2026-01-01 0:00 UTC)),
            end: Some(time::macros::datetime!(2026-01-02 0:00 UTC)),
        };
        let new = ProductListingAuction {
            start: Some(time::macros::datetime!(2026-02-01 0:00 UTC)),
            end: Some(time::macros::datetime!(2026-02-02 0:00 UTC)),
        };
        let mut listing =
            listing_with_state(ProductListingPricing::default(), IndexSet::new(), old);
        listing.take_pending_event_payloads();
        let mut update = command(PatchField::Unchanged);
        update.auction_start = PatchField::Set(new.start.unwrap_or_else(|| panic!("start")));
        update.auction_end = PatchField::Set(new.end.unwrap_or_else(|| panic!("end")));

        apply_update(&mut listing, &update).unwrap_or_else(|error| panic!("update: {error}"));

        assert_eq!(listing.auction(), new);
        assert!(matches!(
            listing.pending_event_payloads(),
            [ProductListingEventPayload::AuctionChanged(change)] if change.auction == new
        ));
    }

    #[test]
    fn should_preserve_absent_title_when_creating_listing() {
        let new_listing = command(PatchField::Unchanged)
            .into_new_product(ProductListingId::new(), test_title_slug())
            .unwrap_or_else(|error| panic!("new listing: {error}"));

        assert!(new_listing.title.is_none());
    }

    #[test]
    fn should_create_listing_without_price_for_clear_or_unchanged_patch() {
        for patch in [
            PatchField::Set(price(100)),
            PatchField::Clear,
            PatchField::Unchanged,
        ] {
            let expected = match &patch {
                PatchField::Set(value) => Some(*value),
                PatchField::Clear | PatchField::Unchanged => None,
            };
            let new_listing = command(patch)
                .into_new_product(ProductListingId::new(), test_title_slug())
                .unwrap_or_else(|error| panic!("new listing: {error}"));
            assert_eq!(expected, new_listing.pricing.price);
        }
    }

    #[test]
    fn should_map_all_patch_states_to_new_listing_current_state() {
        let image = ProductListingImage::new(
            Url::parse("https://example.com/image.jpg")
                .unwrap_or_else(|error| panic!("image URL: {error}")),
        );
        for (
            estimate_min,
            estimate_max,
            images,
            auction_start,
            auction_end,
            expected_min,
            expected_max,
            expected_images,
            expected_auction,
        ) in [
            (
                PatchField::Set(price(110)),
                PatchField::Set(price(120)),
                PatchField::Set(IndexSet::from([image.clone()])),
                PatchField::Set(time::OffsetDateTime::UNIX_EPOCH),
                PatchField::Set(time::OffsetDateTime::UNIX_EPOCH),
                Some(price(110)),
                Some(price(120)),
                IndexSet::from([image.clone()]),
                ProductListingAuction {
                    start: Some(time::OffsetDateTime::UNIX_EPOCH),
                    end: Some(time::OffsetDateTime::UNIX_EPOCH),
                },
            ),
            (
                PatchField::Clear,
                PatchField::Unchanged,
                PatchField::Clear,
                PatchField::Clear,
                PatchField::Unchanged,
                None,
                None,
                IndexSet::new(),
                ProductListingAuction::default(),
            ),
        ] {
            let mut upsert = command(PatchField::Unchanged);
            upsert.price_estimate_min = estimate_min;
            upsert.price_estimate_max = estimate_max;
            upsert.images = images;
            upsert.auction_start = auction_start;
            upsert.auction_end = auction_end;

            let new_listing = upsert
                .into_new_product(ProductListingId::new(), test_title_slug())
                .unwrap_or_else(|error| panic!("new listing: {error}"));

            assert_eq!(new_listing.pricing.price_estimate_min, expected_min);
            assert_eq!(new_listing.pricing.price_estimate_max, expected_max);
            assert_eq!(new_listing.images, expected_images);
            assert_eq!(new_listing.auction, expected_auction);
        }
    }
}
