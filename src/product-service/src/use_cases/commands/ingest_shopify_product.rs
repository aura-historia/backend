use crate::use_cases::{
    UpsertProductCommand, UpsertProductError, UpsertProductResult, UpsertProductUseCase,
};
use common::operation_context::OperationContext;
use common::product_state::domain::ProductState;
use common::shops_product_id::ShopsProductId;
use indexmap::IndexSet;
use localization::Localized;
use money::{MonetaryAmount, Price};
use product_core::description::Description;
use product_core::product::ProductAddress;
use product_core::product_image::ProductImage;
use product_core::prohibited_content::ProhibitedContent;
use product_core::title::Title;
use shop_core::domain::Domain;
use shop_core::partner_status::ShopPartnerStatus;
use shop_service::use_cases::{GetShopError, GetShopRequest, GetShopUseCase};
use url::Url;

#[derive(Debug, Clone, PartialEq)]
pub struct IngestShopifyProductCommand {
    pub shop_domain: Domain,
    pub shops_product_id: ShopsProductId,
    pub title: String,
    pub description: Option<String>,
    pub handle: String,
    pub price: Option<String>,
    pub state: ProductState,
    pub image_urls: IndexSet<Url>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum IngestShopifyProductResult {
    Ignored,
    Upserted(UpsertProductResult),
}

#[derive(Debug, thiserror::Error)]
pub enum IngestShopifyProductError {
    #[error("Shopify product title is missing")]
    MissingTitle,
    #[error("Shopify product handle is missing")]
    MissingHandle,
    #[error("Shopify product price is invalid")]
    InvalidPrice,
    #[error("Shopify product URL is invalid")]
    InvalidProductUrl,
    #[error("Shop has no Shopify language configured")]
    MissingShopLanguage,
    #[error("Shop has no Shopify currency configured")]
    MissingShopCurrency,
    #[error("Shop lookup is temporarily unavailable")]
    ShopLookupTemporarilyUnavailable,
    #[error("Shop lookup returned an invalid read model")]
    InvalidShopReadModel,
    #[error("Shop lookup failed internally")]
    ShopLookupInternal,
    #[error("Shop lookup transaction could not start")]
    ShopLookupBeginTransactionFailed,
    #[error("Shop lookup transaction could not commit")]
    ShopLookupCommitTransactionFailed,
    #[error("product upsert failed")]
    ProductUpsertFailed {
        #[source]
        source: UpsertProductError,
    },
}

#[async_trait::async_trait]
pub trait IngestShopifyProductUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        command: IngestShopifyProductCommand,
    ) -> Result<IngestShopifyProductResult, IngestShopifyProductError>;
}

pub struct IngestShopifyProductHandler<S, U> {
    shops: S,
    products: U,
}

impl<S, U> IngestShopifyProductHandler<S, U> {
    pub fn new(shops: S, products: U) -> Self {
        Self { shops, products }
    }
}

#[async_trait::async_trait]
impl<S, U> IngestShopifyProductUseCase for IngestShopifyProductHandler<S, U>
where
    S: GetShopUseCase,
    U: UpsertProductUseCase,
{
    #[tracing::instrument(
        name = "ingest_shopify_product",
        skip_all,
        fields(
            shop_domain = %command.shop_domain,
            shops_product_id = %command.shops_product_id,
            principal_type = context.principal.kind(),
            request_id = %context.request_id,
            correlation_id = %context.correlation_id,
        )
    )]
    async fn execute(
        &self,
        context: &OperationContext,
        command: IngestShopifyProductCommand,
    ) -> Result<IngestShopifyProductResult, IngestShopifyProductError> {
        let shop = match self
            .shops
            .execute(
                context,
                GetShopRequest::ByShopifyDomain(command.shop_domain.clone()),
            )
            .await
        {
            Ok(shop) => shop,
            Err(GetShopError::NotFound) => return Ok(IngestShopifyProductResult::Ignored),
            Err(error) => return Err(error.into()),
        };
        if shop.partner_status != ShopPartnerStatus::Partnered {
            return Ok(IngestShopifyProductResult::Ignored);
        }

        let language = shop
            .shopify_language
            .ok_or(IngestShopifyProductError::MissingShopLanguage)?;
        let title = command.title.trim();
        if title.is_empty() {
            return Err(IngestShopifyProductError::MissingTitle);
        }
        let handle = command.handle.trim();
        if handle.is_empty() {
            return Err(IngestShopifyProductError::MissingHandle);
        }
        let price = parse_price(command.price.as_deref(), shop.shopify_currency)?;
        let url = Url::parse(&format!(
            "https://{}/products/{handle}",
            command.shop_domain
        ))
        .map_err(|_| IngestShopifyProductError::InvalidProductUrl)?;
        let images = command
            .image_urls
            .into_iter()
            .map(|url| ProductImage {
                url,
                prohibited_content: ProhibitedContent::Unknown,
            })
            .collect();

        self.products
            .execute(
                context,
                UpsertProductCommand {
                    shop_id: shop.shop_id,
                    seller_id: shop.shop_id,
                    shops_product_id: command.shops_product_id,
                    address: ProductAddress::default(),
                    title: Some(Localized::new(language, Title::from(title))),
                    description: command
                        .description
                        .filter(|value| !value.is_empty())
                        .map(Description::from)
                        .map(|value| Localized::new(language, value)),
                    price,
                    price_estimate_min: None,
                    price_estimate_max: None,
                    state: Some(command.state),
                    url: Some(url),
                    images,
                    auction_start: None,
                    auction_end: None,
                },
            )
            .await
            .map(IngestShopifyProductResult::Upserted)
            .map_err(|source| IngestShopifyProductError::ProductUpsertFailed { source })
    }
}

fn parse_price(
    value: Option<&str>,
    currency: Option<money::Currency>,
) -> Result<Option<Price>, IngestShopifyProductError> {
    let Some(value) = value.filter(|value| !value.trim().is_empty()) else {
        return Ok(None);
    };
    let currency = currency.ok_or(IngestShopifyProductError::MissingShopCurrency)?;
    let value = value.trim();
    let (major, minor) = value.split_once('.').unwrap_or((value, ""));
    if !major.chars().all(|value| value.is_ascii_digit())
        || !minor.chars().all(|value| value.is_ascii_digit())
    {
        return Err(IngestShopifyProductError::InvalidPrice);
    }
    let major = major
        .parse::<u64>()
        .map_err(|_| IngestShopifyProductError::InvalidPrice)?;
    let mut minor = minor.chars().take(2).collect::<String>();
    while minor.len() < 2 {
        minor.push('0');
    }
    let minor = minor
        .parse::<u64>()
        .map_err(|_| IngestShopifyProductError::InvalidPrice)?;
    Ok(Some(Price::new(
        MonetaryAmount::from(major * 100 + minor),
        currency,
    )))
}

impl From<GetShopError> for IngestShopifyProductError {
    fn from(error: GetShopError) -> Self {
        match error {
            GetShopError::NotFound => Self::ShopLookupInternal,
            GetShopError::TemporarilyUnavailable { .. } => Self::ShopLookupTemporarilyUnavailable,
            GetShopError::InvalidReadModel { .. } => Self::InvalidShopReadModel,
            GetShopError::Internal { .. } => Self::ShopLookupInternal,
            GetShopError::BeginTransactionFailed => Self::ShopLookupBeginTransactionFailed,
            GetShopError::CommitTransactionFailed => Self::ShopLookupCommitTransactionFailed,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use common::operation_context::{CorrelationId, Principal, RequestId};
    use localization::Language;
    use money::Currency;
    use shop_core::shop_id::ShopId;
    use shop_core::shop_name::ShopName;
    use shop_core::shop_type::ShopType;
    use shop_service::use_cases::ShopDetailsView;
    use std::collections::HashSet;
    use std::sync::{Arc, Mutex};
    use time::OffsetDateTime;

    #[tokio::test]
    async fn should_upsert_for_partner_shop_with_resolved_shop_identity() {
        let shop_id = ShopId::new();
        let products = FakeProducts::default();
        let result = IngestShopifyProductHandler::new(
            FakeShops::new(shop(shop_id, ShopPartnerStatus::Partnered)),
            products.clone(),
        )
        .execute(&context(), command())
        .await;

        assert!(matches!(
            result,
            Ok(IngestShopifyProductResult::Upserted(_))
        ));
        let command = products
            .command
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clone();
        assert!(
            matches!(command, Some(command) if command.shop_id == shop_id && command.seller_id == shop_id)
        );
    }

    #[tokio::test]
    async fn should_ignore_non_partner_shop_without_product_upsert() {
        let products = FakeProducts::default();
        let result = IngestShopifyProductHandler::new(
            FakeShops::new(shop(ShopId::new(), ShopPartnerStatus::Scraped)),
            products.clone(),
        )
        .execute(&context(), command())
        .await;

        assert!(matches!(result, Ok(IngestShopifyProductResult::Ignored)));
        assert!(
            products
                .command
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .is_none()
        );
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
            Err(IngestShopifyProductError::MissingShopCurrency)
        ));
    }

    #[test]
    fn should_reject_invalid_shopify_price() {
        assert!(matches!(
            parse_price(Some("invalid"), Some(Currency::Eur)),
            Err(IngestShopifyProductError::InvalidPrice)
        ));
    }

    #[derive(Clone)]
    struct FakeShops {
        shop: ShopDetailsView,
    }

    impl FakeShops {
        fn new(shop: ShopDetailsView) -> Self {
            Self { shop }
        }
    }

    #[async_trait::async_trait]
    impl GetShopUseCase for FakeShops {
        async fn execute(
            &self,
            _context: &OperationContext,
            _request: GetShopRequest,
        ) -> Result<ShopDetailsView, GetShopError> {
            Ok(self.shop.clone())
        }
    }

    #[derive(Clone, Default)]
    struct FakeProducts {
        command: Arc<Mutex<Option<UpsertProductCommand>>>,
    }

    #[async_trait::async_trait]
    impl UpsertProductUseCase for FakeProducts {
        async fn execute(
            &self,
            _context: &OperationContext,
            command: UpsertProductCommand,
        ) -> Result<UpsertProductResult, UpsertProductError> {
            *self
                .command
                .lock()
                .unwrap_or_else(|error| error.into_inner()) = Some(command);
            Ok(UpsertProductResult::Created(
                crate::use_cases::CreateProductResult {
                    product_id: common::product_id::ProductId::new(),
                    product_slug_id: "shopify-product".into(),
                    event_id: common::event_id::EventId::new(),
                },
            ))
        }
    }

    fn command() -> IngestShopifyProductCommand {
        IngestShopifyProductCommand {
            shop_domain: Domain::try_from("partner.example")
                .unwrap_or_else(|error| panic!("invalid domain: {error}")),
            shops_product_id: ShopsProductId::from("shopify-1"),
            title: "Cabinet".to_owned(),
            description: Some("Cabinet description".to_owned()),
            handle: "cabinet".to_owned(),
            price: Some("42.00".to_owned()),
            state: ProductState::Available,
            image_urls: IndexSet::new(),
        }
    }

    fn context() -> OperationContext {
        OperationContext {
            principal: Principal::System,
            request_id: RequestId::new("request"),
            correlation_id: CorrelationId::new("correlation"),
        }
    }

    fn shop(shop_id: ShopId, partner_status: ShopPartnerStatus) -> ShopDetailsView {
        ShopDetailsView {
            shop_id,
            shop_slug_id: "partner".into(),
            name: ShopName::from("Partner"),
            shop_type: ShopType::CommercialDealer,
            domains: HashSet::new(),
            shopify_domain: Some(
                Domain::try_from("partner.example")
                    .unwrap_or_else(|error| panic!("invalid domain: {error}")),
            ),
            shopify_currency: Some(Currency::Usd),
            shopify_language: Some(Language::De),
            woocommerce_currency: None,
            woocommerce_language: None,
            url: None,
            view_url: None,
            image: None,
            structured_address: None,
            geo_address: None,
            phone: None,
            email: None,
            partner_status,
            affiliate_configuration: None,
            created: OffsetDateTime::UNIX_EPOCH,
            updated: OffsetDateTime::UNIX_EPOCH,
        }
    }
}
