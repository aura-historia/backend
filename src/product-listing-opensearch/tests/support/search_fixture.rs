//! Shared documents and search helpers for the real OpenSearch contract cases.
use super::*;

pub(super) async fn search(
    search: ProductListingSearch,
    price_filter: ProductListingPriceFilterPlan,
) -> Result<
    ProductListingSearchReadResult,
    product_listing_service::ports::ProductListingSearchReadError,
> {
    let reader = product_listing_opensearch::OpenSearchProductListingSearchReader::with_index(
        get_opensearch_client().await.clone(),
        PRODUCTS_INDEX,
    );
    reader
        .search(&ProductListingSearchReadRequest {
            compiled_search: CompiledProductListingSearch::new(search, price_filter)
                .expect("test search compiles"),
            sort: None,
            cursor: Some(Cursor {
                size: 20,
                search_after: None,
            }),
        })
        .await
}

pub(super) fn price_filter(
    range: Option<RangeQuery<MonetaryAmount>>,
) -> Result<ProductListingPriceFilterPlan, Box<dyn std::error::Error + Send + Sync>> {
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
    Ok(ProductListingPriceFilterPlan::compile(
        snapshot,
        Currency::Usd,
        range,
    )?)
}

pub(super) async fn index_products(
    product_listings: impl IntoIterator<Item = Value>,
) -> TestResult {
    let client = get_opensearch_client().await;
    for product in product_listings {
        let id = product_listing_id_string(&product)?;
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

fn product_listing_id_string(product: &Value) -> Result<String, IoError> {
    product
        .get("productListingId")
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
        .ok_or_else(|| IoError::new(ErrorKind::InvalidData, "productListingId missing"))
}

pub(super) fn assert_ok(result: TestResult) {
    assert_eq!(Ok(()), result.map_err(|error| error.to_string()));
}

pub(super) struct ProductListingSeed {
    product_listing_id: ProductListingId,
    product_listing_title_slug_id: ProductListingSlugId,
    source_price: Option<u64>,
    sale_price: Option<u64>,
    has_sale_observation: bool,
}

impl ProductListingSeed {
    pub(super) fn active(slug: &str, source_price: u64) -> Self {
        Self {
            product_listing_id: ProductListingId::new(),
            product_listing_title_slug_id: ProductListingSlugId::from_title_and_suffix(
                slug, "a1b2c3",
            )
            .unwrap_or_else(|error| panic!("valid product listing title slug: {error}")),
            source_price: Some(source_price),
            sale_price: None,
            has_sale_observation: false,
        }
    }

    pub(super) fn sold(slug: &str, sale_price: u64) -> Self {
        Self {
            product_listing_id: ProductListingId::new(),
            product_listing_title_slug_id: ProductListingSlugId::from_title_and_suffix(
                slug, "a1b2c3",
            )
            .unwrap_or_else(|error| panic!("valid product listing title slug: {error}")),
            source_price: Some(sale_price),
            sale_price: Some(sale_price),
            has_sale_observation: true,
        }
    }

    pub(super) fn sold_without_main_price(slug: &str) -> Self {
        Self {
            product_listing_id: ProductListingId::new(),
            product_listing_title_slug_id: ProductListingSlugId::from_title_and_suffix(
                slug, "a1b2c3",
            )
            .unwrap_or_else(|error| panic!("valid product listing title slug: {error}")),
            source_price: None,
            sale_price: None,
            has_sale_observation: true,
        }
    }
}

pub(super) struct IndexedProductListing {
    pub(super) document: Value,
}

pub(super) fn product_listing_document(
    seed: ProductListingSeed,
) -> Result<IndexedProductListing, time::error::Format> {
    let sale_prices = seed.sale_price.map(|amount| {
        json!({
            "eur": amount, "gbp": amount, "usd": amount, "aud": amount,
            "cad": amount, "nzd": amount, "cny": amount, "brl": amount,
            "pln": amount, "try": amount, "jpy": amount, "czk": amount,
            "rub": amount, "aed": amount, "sar": amount, "hkd": amount,
            "sgd": amount, "chf": amount, "zar": amount, "sek": amount, "dkk": amount, "nok": amount, "krw": amount, "inr": amount, "twd": amount, "huf": amount, "ron": amount, "mxn": amount, "thb": amount, "ars": amount
        })
    });
    let sale_observed_at = seed
        .has_sale_observation
        .then(|| OffsetDateTime::UNIX_EPOCH.format(&Rfc3339))
        .transpose()?;
    let document = json!({
        "productListingId": seed.product_listing_id,
        "productListingTitleSlugId": seed.product_listing_title_slug_id,
        "listingSourceId": ListingSourceId::new().to_string(),
        "sourceListingId": SourceListingId::try_from("sku-1")
                    .unwrap_or_else(|error| panic!("valid source listing ID: {error}")),
        "eventId": EventId::new(),
        "title": { "text": "Blue vase", "language": "EN" },
        "titleEn": "Blue vase",
        "sourcePrice": seed.source_price.map(|amount| json!({
                    "type": "MONETARY",
                    "amount": amount,
                    "currency": "EUR"
                })),
        "salePrices": sale_prices,
        "saleObservationFxRateId": seed.has_sale_observation.then(FxRateId::new),
        "saleObservedAt": sale_observed_at,

        "url": format!("https://shop.example/product_listings/{}", seed.product_listing_title_slug_id),
        "created": OffsetDateTime::UNIX_EPOCH.format(&Rfc3339)?,
        "updated": OffsetDateTime::UNIX_EPOCH.format(&Rfc3339)?
    });

    Ok(IndexedProductListing { document })
}
