use common::currency::domain::Currency;
use common::event_id::EventId;
use common::fx_rate_id::FxRateId;
use common::pagination::cursor::Cursor;
use common::price::domain::MonetaryAmount;
use common::product_id::ProductId;
use common::product_slug_id::ProductSlugId;
use common::query::range_query::RangeQuery;
use common::seller_slug_id::SellerSlugId;
use common::shop_id::ShopId;
use common::shop_slug_id::ShopSlugId;
use common::shops_product_id::ShopsProductId;
use opensearch::IndexParts;
use product_core::product_search::ProductSearch;
use product_service::ports::{
    CompiledProductSearch, ProductPriceFilterPlan, ProductSearchReadRequest, ProductSearchReader,
};
use product_service::use_cases::queries::search_products::ProductSearchReadResult;
use serde_json::{Value, json};
use std::io::{Error as IoError, ErrorKind};
use strum::IntoEnumIterator;
use test_api::{
    IntegrationTestService, OpenSearch, aura_integration_test, get_opensearch_client, refresh_index,
};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

const PRODUCTS_INDEX: &str = "products";

type TestResult = Result<(), Box<dyn std::error::Error + Send + Sync>>;

#[aura_integration_test(services = [OpenSearch()])]
async fn should_search_active_and_sold_products_with_one_pinned_price_plan() {
    assert_ok(should_search_active_and_sold_products_with_one_pinned_price_plan_impl().await);
}

async fn should_search_active_and_sold_products_with_one_pinned_price_plan_impl() -> TestResult {
    let active = product_document(ProductSeed::active("active", 100))?;
    let sold = product_document(ProductSeed::sold("sold", 110))?;
    index_products([active.document, sold.document]).await?;

    let result = search(
        ProductSearch::new(common::language::domain::Language::En, Currency::Usd).with_price_query(
            RangeQuery {
                min: Some(MonetaryAmount::from(110_u64)),
                max: Some(MonetaryAmount::from(110_u64)),
            },
        ),
        price_filter(Some(RangeQuery {
            min: Some(MonetaryAmount::from(110_u64)),
            max: Some(MonetaryAmount::from(110_u64)),
        }))?,
    )
    .await?;

    assert_eq!(Some(2), result.total);
    assert_eq!(2, result.items.len());
    assert!(result.items.iter().all(|item| {
        item.display_price
            == Some(common::price::domain::Price::new(
                MonetaryAmount::from(110_u64),
                Currency::Usd,
            ))
    }));
    Ok(())
}

#[aura_integration_test(services = [OpenSearch()])]
async fn should_return_sold_product_without_main_price_for_non_price_search_and_exclude_it_from_price_search()
 {
    assert_ok(
        should_return_sold_product_without_main_price_for_non_price_search_and_exclude_it_from_price_search_impl()
            .await,
    );
}

async fn should_return_sold_product_without_main_price_for_non_price_search_and_exclude_it_from_price_search_impl()
-> TestResult {
    let sold_without_price =
        product_document(ProductSeed::sold_without_main_price("sold-no-price"))?;
    let product_id = sold_without_price.document["productId"]
        .as_str()
        .ok_or_else(|| IoError::new(ErrorKind::InvalidData, "productId missing"))?
        .to_owned();
    index_products([sold_without_price.document]).await?;

    let non_price_result = search(
        ProductSearch::new(common::language::domain::Language::En, Currency::Usd),
        price_filter(None)?,
    )
    .await?;
    assert_eq!(Some(1), non_price_result.total);
    assert_eq!(1, non_price_result.items.len());
    assert_eq!(product_id, non_price_result.items[0].product_id.to_string());
    assert_eq!(None, non_price_result.items[0].display_price);
    assert!(matches!(
        non_price_result.items[0].price_valuation,
        product_service::use_cases::ProductSummaryPriceValuation::Sale { .. }
    ));

    let price_result = search(
        ProductSearch::new(common::language::domain::Language::En, Currency::Usd).with_price_query(
            RangeQuery {
                min: Some(MonetaryAmount::from(1_u64)),
                max: Some(MonetaryAmount::from(1_000_u64)),
            },
        ),
        price_filter(Some(RangeQuery {
            min: Some(MonetaryAmount::from(1_u64)),
            max: Some(MonetaryAmount::from(1_000_u64)),
        }))?,
    )
    .await?;
    assert_eq!(Some(0), price_result.total);
    assert!(price_result.items.is_empty());
    Ok(())
}

async fn search(
    search: ProductSearch,
    price_filter: ProductPriceFilterPlan,
) -> Result<ProductSearchReadResult, product_service::ports::ProductSearchReadError> {
    let reader = product_opensearch::OpenSearchProductSearchReader::new(
        get_opensearch_client().await.clone(),
    );
    reader
        .search(&ProductSearchReadRequest {
            compiled_search: CompiledProductSearch {
                search,
                price_filter_plan: price_filter,
            },
            sort: None,
            cursor: Some(Cursor {
                size: 20,
                search_after: None,
            }),
        })
        .await
}

fn price_filter(
    range: Option<RangeQuery<MonetaryAmount>>,
) -> Result<ProductPriceFilterPlan, Box<dyn std::error::Error + Send + Sync>> {
    use fxrate_core::{FX_RATE_SCALE, FxRateQuote, FxRateSource, NewFxRateSnapshot};

    let snapshot = NewFxRateSnapshot::capture_eur(
        FxRateId::new(),
        OffsetDateTime::UNIX_EPOCH,
        FxRateSource::FxRatesApi,
        Currency::Eur,
        Currency::iter().map(|currency| {
            FxRateQuote::new(
                currency,
                if currency == Currency::Usd {
                    1_100_000
                } else {
                    FX_RATE_SCALE
                },
            )
        }),
    )?
    .into_persisted(1_i64.try_into()?);
    Ok(ProductPriceFilterPlan::compile(
        snapshot,
        Currency::Usd,
        range,
    )?)
}

async fn index_products(products: impl IntoIterator<Item = Value>) -> TestResult {
    let client = get_opensearch_client().await;
    for product in products {
        let id = product_id_string(&product)?;
        let response = client
            .index(IndexParts::IndexId(PRODUCTS_INDEX, &id))
            .body(product)
            .send()
            .await?;
        let status = response.status_code();
        let payload = response.text().await?;
        if !status.is_success() {
            return Err(IoError::new(
                ErrorKind::InvalidData,
                format!("failed indexing product {id}: HTTP {status}: {payload}"),
            )
            .into());
        }
    }
    refresh_index(PRODUCTS_INDEX).await;
    Ok(())
}

fn product_id_string(product: &Value) -> Result<String, IoError> {
    product
        .get("productId")
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
        .ok_or_else(|| IoError::new(ErrorKind::InvalidData, "productId missing"))
}

fn assert_ok(result: TestResult) {
    assert_eq!(Ok(()), result.map_err(|error| error.to_string()));
}

struct ProductSeed {
    product_id: ProductId,
    product_slug_id: ProductSlugId,
    source_price: Option<u64>,
    sale_price: Option<u64>,
    has_sale_valuation: bool,
}

impl ProductSeed {
    fn active(slug: &str, source_price: u64) -> Self {
        Self {
            product_id: ProductId::new(),
            product_slug_id: ProductSlugId::from(slug),
            source_price: Some(source_price),
            sale_price: None,
            has_sale_valuation: false,
        }
    }

    fn sold(slug: &str, sale_price: u64) -> Self {
        Self {
            product_id: ProductId::new(),
            product_slug_id: ProductSlugId::from(slug),
            source_price: None,
            sale_price: Some(sale_price),
            has_sale_valuation: true,
        }
    }

    fn sold_without_main_price(slug: &str) -> Self {
        Self {
            product_id: ProductId::new(),
            product_slug_id: ProductSlugId::from(slug),
            source_price: None,
            sale_price: None,
            has_sale_valuation: true,
        }
    }
}

struct IndexedProduct {
    document: Value,
}

fn product_document(seed: ProductSeed) -> Result<IndexedProduct, time::error::Format> {
    let shop_id = ShopId::new();
    let sale_prices = seed.sale_price.map(|amount| {
        json!({
            "eur": amount, "gbp": amount, "usd": amount, "aud": amount,
            "cad": amount, "nzd": amount, "cny": amount, "brl": amount,
            "pln": amount, "try": amount, "jpy": amount, "czk": amount,
            "rub": amount, "aed": amount, "sar": amount, "hkd": amount,
            "sgd": amount, "chf": amount
        })
    });
    let sold_at = seed
        .has_sale_valuation
        .then(|| OffsetDateTime::UNIX_EPOCH.format(&Rfc3339))
        .transpose()?;
    let document = json!({
        "productId": seed.product_id,
        "productSlugId": seed.product_slug_id,
        "shopSlugId": ShopSlugId::from("shop"),
        "sellerSlugId": SellerSlugId::from("seller"),
        "eventId": EventId::new(),
        "shopId": shop_id,
        "sellerId": shop_id,
        "shopsProductId": ShopsProductId::from("sku-1"),
        "shopName": "Shop",
        "sellerName": "Seller",
        "shopType": "COMMERCIAL_DEALER",
        "title": { "text": "Blue vase", "language": "EN" },
        "titleEn": "Blue vase",
        "sourcePrice": seed.source_price.map(|amount| json!({ "amount": amount, "currency": "EUR" })),
        "salePrices": sale_prices,
        "saleFxRateId": seed.has_sale_valuation.then(FxRateId::new),
        "soldAt": sold_at,
        "state": if seed.has_sale_valuation { "SOLD" } else { "AVAILABLE" },
        "lifecycle": "ACTIVE",
        "url": format!("https://shop.example/products/{}", seed.product_slug_id),
        "viewUrl": format!("https://aura.example/products/{}", seed.product_slug_id),
        "created": OffsetDateTime::UNIX_EPOCH.format(&Rfc3339)?,
        "updated": OffsetDateTime::UNIX_EPOCH.format(&Rfc3339)?
    });

    Ok(IndexedProduct { document })
}
