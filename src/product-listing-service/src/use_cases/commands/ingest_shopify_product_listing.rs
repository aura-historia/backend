use crate::use_cases::{
    UpsertProductListingCommand, UpsertProductListingError, UpsertProductListingResult,
    UpsertProductListingUseCase,
};
use application::operation_context::OperationContext;
use application::patch_field::PatchField;
use indexmap::IndexSet;
use listing_source_core::Domain;
use listing_source_service::use_cases::queries::get_shopify_source::{
    GetShopifySourceError, GetShopifySourceRequest, GetShopifySourceUseCase,
};
use localization::Localized;
use money::{MonetaryAmount, Price};
use product_listing_core::{
    description::Description, listing_availability::ListingAvailability,
    product_listing_image::ProductListingImage, source_listing_id::SourceListingId, title::Title,
};
use url::Url;

#[derive(Debug, Clone, PartialEq)]
pub struct IngestShopifyProductListingCommand {
    pub source_domain: Domain,
    pub source_listing_id: SourceListingId,
    pub title: String,
    pub description: Option<String>,
    pub handle: String,
    pub price: Option<String>,
    pub availability: PatchField<ListingAvailability>,
    pub image_urls: IndexSet<Url>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum IngestShopifyProductListingResult {
    Ignored,
    Upserted(UpsertProductListingResult),
}

#[derive(Debug, thiserror::Error)]
pub enum IngestShopifyProductListingError {
    #[error("Shopify product title is missing")]
    MissingTitle,
    #[error("Shopify product handle is missing")]
    MissingHandle,
    #[error("Shopify product price is invalid")]
    InvalidPrice,
    #[error("Shopify product URL is invalid")]
    InvalidProductListingUrl,
    #[error("listing source has no Shopify language configured")]
    MissingListingSourceLanguage,
    #[error("listing source has no Shopify currency configured")]
    MissingListingSourceCurrency,
    #[error("listing source lookup is temporarily unavailable")]
    ListingSourceLookupTemporarilyUnavailable,
    #[error("listing source lookup returned an invalid read model")]
    InvalidListingSourceReadModel,
    #[error("listing source partnership grant is required")]
    ListingSourcePartnershipGrantRequired,
    #[error("product upsert failed")]
    ProductListingUpsertFailed {
        #[source]
        source: UpsertProductListingError,
    },
}

#[async_trait::async_trait]
pub trait IngestShopifyProductListingUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        command: IngestShopifyProductListingCommand,
    ) -> Result<IngestShopifyProductListingResult, IngestShopifyProductListingError>;
}

pub struct IngestShopifyProductListingHandler<S, U> {
    sources: S,
    products: U,
}

impl<S, U> IngestShopifyProductListingHandler<S, U> {
    pub fn new(sources: S, products: U) -> Self {
        Self { sources, products }
    }
}

#[async_trait::async_trait]
impl<S, U> IngestShopifyProductListingUseCase for IngestShopifyProductListingHandler<S, U>
where
    S: GetShopifySourceUseCase,
    U: UpsertProductListingUseCase,
{
    #[tracing::instrument(
        name = "ingest_shopify_product",
        skip_all,
        fields(
            source_domain = %command.source_domain,
            source_listing_id = %command.source_listing_id,
            principal_type = context.principal.kind(),
            request_id = %context.request_id,
            correlation_id = %context.correlation_id,
        )
    )]
    async fn execute(
        &self,
        context: &OperationContext,
        command: IngestShopifyProductListingCommand,
    ) -> Result<IngestShopifyProductListingResult, IngestShopifyProductListingError> {
        let source = match self
            .sources
            .execute(
                context,
                GetShopifySourceRequest {
                    domain: command.source_domain.clone(),
                },
            )
            .await
        {
            Ok(source) => source,
            Err(GetShopifySourceError::NotFound) => {
                return Ok(IngestShopifyProductListingResult::Ignored);
            }
            Err(error) => return Err(error.into()),
        };

        let language = source
            .language
            .ok_or(IngestShopifyProductListingError::MissingListingSourceLanguage)?;
        let title = command.title.trim();
        if title.is_empty() {
            return Err(IngestShopifyProductListingError::MissingTitle);
        }
        let handle = command.handle.trim();
        if handle.is_empty() {
            return Err(IngestShopifyProductListingError::MissingHandle);
        }
        let price = parse_price(command.price.as_deref(), source.currency)?;
        let url = Url::parse(&format!(
            "https://{}/products/{handle}",
            command.source_domain
        ))
        .map_err(|_| IngestShopifyProductListingError::InvalidProductListingUrl)?;
        let images = command
            .image_urls
            .into_iter()
            .map(ProductListingImage::new)
            .collect();

        self.products
            .execute(
                context,
                UpsertProductListingCommand {
                    listing_source_id: source.listing_source_id,
                    source_listing_id: command.source_listing_id,
                    title: Some(Localized::new(language, Title::from(title))),
                    description: command
                        .description
                        .filter(|value| !value.is_empty())
                        .map(Description::from)
                        .map(|value| Localized::new(language, value)),
                    price: match price {
                        Some(price) => PatchField::Set(price),
                        None => PatchField::Clear,
                    },
                    price_estimate_min: PatchField::Unchanged,
                    price_estimate_max: PatchField::Unchanged,
                    availability: command.availability,
                    url: Some(url),
                    images: PatchField::Set(images),
                    auction_start: PatchField::Unchanged,
                    auction_end: PatchField::Unchanged,
                },
            )
            .await
            .map(IngestShopifyProductListingResult::Upserted)
            .map_err(
                |source| IngestShopifyProductListingError::ProductListingUpsertFailed { source },
            )
    }
}

fn parse_price(
    value: Option<&str>,
    currency: Option<money::Currency>,
) -> Result<Option<Price>, IngestShopifyProductListingError> {
    let Some(value) = value.filter(|value| !value.trim().is_empty()) else {
        return Ok(None);
    };
    let currency =
        currency.ok_or(IngestShopifyProductListingError::MissingListingSourceCurrency)?;
    let value = value.trim();
    let (major, minor) = value.split_once('.').unwrap_or((value, ""));
    if !major.chars().all(|value| value.is_ascii_digit())
        || !minor.chars().all(|value| value.is_ascii_digit())
    {
        return Err(IngestShopifyProductListingError::InvalidPrice);
    }
    let major = major
        .parse::<u64>()
        .map_err(|_| IngestShopifyProductListingError::InvalidPrice)?;
    let mut minor = minor.chars().take(2).collect::<String>();
    while minor.len() < 2 {
        minor.push('0');
    }
    let minor = minor
        .parse::<u64>()
        .map_err(|_| IngestShopifyProductListingError::InvalidPrice)?;
    Ok(Some(Price::new(
        MonetaryAmount::from(major * 100 + minor),
        currency,
    )))
}

impl From<GetShopifySourceError> for IngestShopifyProductListingError {
    fn from(error: GetShopifySourceError) -> Self {
        match error {
            GetShopifySourceError::NotFound => Self::InvalidListingSourceReadModel,
            GetShopifySourceError::PartnershipGrantRequired => {
                Self::ListingSourcePartnershipGrantRequired
            }
            GetShopifySourceError::TemporarilyUnavailable { .. } => {
                Self::ListingSourceLookupTemporarilyUnavailable
            }
            GetShopifySourceError::InvalidReadModel { .. } => Self::InvalidListingSourceReadModel,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use application::operation_context::{CorrelationId, Principal, RequestId};
    use listing_source_core::ListingSourceId;
    use listing_source_service::ports::ShopifySource;
    use localization::Language;
    use money::Currency;

    use std::sync::{Arc, Mutex};

    #[tokio::test]
    async fn should_upsert_for_resolved_listing_source_identity() {
        let listing_source_id = ListingSourceId::new();
        let products = FakeProducts::default();
        let result = IngestShopifyProductListingHandler::new(
            FakeSources::found(source(listing_source_id)),
            products.clone(),
        )
        .execute(&context(), command())
        .await;

        assert!(matches!(
            result,
            Ok(IngestShopifyProductListingResult::Upserted(_))
        ));
        let command = lock(&products.command).clone();
        assert!(matches!(
            command,
            Some(command)
                if command.listing_source_id == listing_source_id
                    && matches!(&command.price_estimate_min, PatchField::Unchanged)
                    && matches!(&command.price_estimate_max, PatchField::Unchanged)
                    && matches!(&command.auction_start, PatchField::Unchanged)
                    && matches!(&command.auction_end, PatchField::Unchanged)
                    && matches!(&command.images, PatchField::Set(images) if images.is_empty())
        ));
    }

    #[tokio::test]
    async fn should_ignore_missing_listing_source_without_product_upsert() {
        let products = FakeProducts::default();
        let result =
            IngestShopifyProductListingHandler::new(FakeSources::missing(), products.clone())
                .execute(&context(), command())
                .await;

        assert!(matches!(
            result,
            Ok(IngestShopifyProductListingResult::Ignored)
        ));
        assert!(lock(&products.command).is_none());
    }

    #[test]
    fn should_parse_shopify_price_with_two_minor_digits() {
        let price = parse_price(Some("42.5"), Some(Currency::Usd));

        assert!(matches!(
            price,
            Ok(Some(value)) if value == Price::new(MonetaryAmount::from(4_250_u64), Currency::Usd)
        ));
    }

    #[test]
    fn should_require_currency_only_when_price_exists() {
        assert!(matches!(parse_price(None, None), Ok(None)));
        assert!(matches!(
            parse_price(Some("42.00"), None),
            Err(IngestShopifyProductListingError::MissingListingSourceCurrency)
        ));
    }

    #[derive(Clone)]
    struct FakeSources(Option<ShopifySource>);

    impl FakeSources {
        fn found(source: ShopifySource) -> Self {
            Self(Some(source))
        }

        fn missing() -> Self {
            Self(None)
        }
    }

    #[async_trait::async_trait]
    impl GetShopifySourceUseCase for FakeSources {
        async fn execute(
            &self,
            _: &OperationContext,
            _: GetShopifySourceRequest,
        ) -> Result<ShopifySource, GetShopifySourceError> {
            self.0.clone().ok_or(GetShopifySourceError::NotFound)
        }
    }

    #[derive(Clone, Default)]
    struct FakeProducts {
        command: Arc<Mutex<Option<UpsertProductListingCommand>>>,
    }

    #[async_trait::async_trait]
    impl UpsertProductListingUseCase for FakeProducts {
        async fn execute(
            &self,
            _: &OperationContext,
            command: UpsertProductListingCommand,
        ) -> Result<UpsertProductListingResult, UpsertProductListingError> {
            *lock(&self.command) = Some(command);
            Ok(UpsertProductListingResult::Created(
                crate::use_cases::CreateProductListingResult {
                    product_listing_id:
                        product_listing_core::product_listing_id::ProductListingId::new(),
                    product_listing_title_slug_id: "shopify-product".into(),
                    event_id: domain_primitives::event_id::EventId::new(),
                },
            ))
        }
    }

    fn command() -> IngestShopifyProductListingCommand {
        IngestShopifyProductListingCommand {
            source_domain: Domain::try_from("partner.example")
                .unwrap_or_else(|error| panic!("invalid test domain: {error}")),
            source_listing_id: SourceListingId::try_from("shopify-1")
                .unwrap_or_else(|error| panic!("valid source listing ID: {error}")),
            title: "Cabinet".to_owned(),
            description: Some("Cabinet description".to_owned()),
            handle: "cabinet".to_owned(),
            price: Some("42.00".to_owned()),
            availability: PatchField::Set(ListingAvailability::InStock),
            image_urls: IndexSet::new(),
        }
    }

    fn source(listing_source_id: ListingSourceId) -> ShopifySource {
        ShopifySource {
            listing_source_id,

            domain: Domain::try_from("partner.example")
                .unwrap_or_else(|error| panic!("invalid test domain: {error}")),
            currency: Some(Currency::Usd),
            language: Some(Language::De),
        }
    }

    fn context() -> OperationContext {
        OperationContext {
            principal: Principal::System,
            request_id: RequestId::new("request"),
            correlation_id: CorrelationId::new("correlation"),
        }
    }

    fn lock<T>(value: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
        match value.lock() {
            Ok(value) => value,
            Err(poisoned) => poisoned.into_inner(),
        }
    }
}
