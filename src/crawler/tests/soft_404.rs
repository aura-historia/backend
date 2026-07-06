use async_trait::async_trait;
use common::shop_id::ShopId;
use crawler::scraper::candidate_service::ScraperCandidateServiceImpl;
use crawler::scraper::css_selector::product_schema::{
    ApplySchemaError, ProductCssSelectorSchema, RawExtractedProduct, ShopsProductSchema,
};
use crawler::scraper::css_selector::product_schema_service::{
    GeneratedProductSchemas, ProductSchemaService, ProductSchemaServiceError,
};
use crawler::scraper::normalization::product_normalization_service::{
    ProductNormalizationResult, ProductNormalizationService,
};
use crawler::scraper::scraper_service::{
    ReqwestHtmlFetcher, ScraperError, ScraperService, ScraperServiceImpl,
};
use crawler::spider::classification::url_metadata::UrlClass;
use crawler::spider::classification::url_metadata_repository::{
    UrlMetadataRepository, UrlMetadataRepositoryImpl,
};
use std::sync::{Arc, Mutex};
use test_api::*;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use url::Url;

const RDS: Rds = Rds {
    migrations_dir: "src/crawler/migrations",
};

struct PanicSchemaService;

#[async_trait]
impl ProductSchemaService for PanicSchemaService {
    async fn create_product_schema(
        &self,
        _html_pages: &[String],
    ) -> Result<ProductCssSelectorSchema, ProductSchemaServiceError> {
        panic!("schema service should not be called for soft 404")
    }

    async fn create_product_schemas(
        &self,
        _html_pages: &[String],
    ) -> Result<GeneratedProductSchemas, ProductSchemaServiceError> {
        panic!("schema service should not be called for soft 404")
    }

    async fn append_single_schema(
        &self,
        _html: &str,
        _failed_schema: Option<&ProductCssSelectorSchema>,
        _last_error: Option<&ApplySchemaError>,
    ) -> Result<GeneratedProductSchemas, ProductSchemaServiceError> {
        panic!("schema service should not be called for soft 404")
    }

    async fn find_product_schema(
        &self,
        _shop_id: &ShopId,
    ) -> Result<Option<ShopsProductSchema>, ProductSchemaServiceError> {
        panic!("schema service should not be called for soft 404")
    }

    async fn save_product_schema(
        &self,
        _shop_id: &ShopId,
        _product_schema: ProductCssSelectorSchema,
    ) -> Result<ShopsProductSchema, ProductSchemaServiceError> {
        panic!("schema service should not be called for soft 404")
    }

    async fn save_product_schemas(
        &self,
        _shop_id: &ShopId,
        _product_schemas: Vec<ProductCssSelectorSchema>,
    ) -> Result<ShopsProductSchema, ProductSchemaServiceError> {
        panic!("schema service should not be called for soft 404")
    }

    async fn get_product_schema(
        &self,
        _shop_id: &ShopId,
        _html_pages: &[String],
    ) -> Result<ShopsProductSchema, ProductSchemaServiceError> {
        panic!("schema service should not be called for soft 404")
    }
}

struct PanicNormalizationService;

#[async_trait]
impl ProductNormalizationService for PanicNormalizationService {
    async fn normalize(
        &self,
        _raw: RawExtractedProduct,
        _url: Url,
        _default_currency: Option<common::currency::domain::Currency>,
    ) -> ProductNormalizationResult {
        panic!("normalization service should not be called for soft 404")
    }
}

const HOMEPAGE_HTML: &str = r#"
<!DOCTYPE html>
<html>
  <body>
    <nav>Home Shop About Contact</nav>
    <main><h1>Welcome to the shop</h1><p>Browse our latest antique products.</p></main>
  </body>
</html>
"#;

const CANONICAL_PRODUCT_HTML: &str = r#"
<!DOCTYPE html>
<html>
  <head>
    <link rel="canonical" href="/antique-maps/vintage-sea-chart-map-of-st-ives-to-dodman-point/itm284170">
    <meta property="og:url" content="/antique-maps/vintage-sea-chart-map-of-st-ives-to-dodman-point/itm284170">
    <title>Vintage Sea Chart Map</title>
  </head>
  <body>
    <script type="application/ld+json">{"@type":"Product","name":"Vintage Sea Chart Map"}</script>
    <main><h1>Vintage Sea Chart Map</h1><p>Available now</p></main>
  </body>
</html>
"#;

async fn spawn_soft_404_server(html: &'static str) -> (Url, Arc<Mutex<Vec<String>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let requested_paths = Arc::new(Mutex::new(Vec::new()));
    let server_requested_paths = requested_paths.clone();

    tokio::spawn(async move {
        for _ in 0..5 {
            let Ok((mut socket, _)) = listener.accept().await else {
                return;
            };
            let mut buffer = [0_u8; 2048];
            let bytes_read = socket.read(&mut buffer).await.unwrap_or_default();
            let request = String::from_utf8_lossy(&buffer[..bytes_read]);
            let path = request
                .lines()
                .next()
                .and_then(|line| line.split_whitespace().nth(1))
                .unwrap_or("/");
            if let Ok(mut paths) = server_requested_paths.lock() {
                paths.push(path.to_string());
            }
            let response = soft_404_response_for(path, html);
            let _ = socket.write_all(response.as_bytes()).await;
        }
    });

    let product_url = Url::parse(&format!(
        "http://{addr}/antique-maps/vintage-sea-chart-map-of-st-ives-to-dodman-point/itm284170"
    ))
    .unwrap();

    (product_url, requested_paths)
}

fn soft_404_response_for(path: &str, html: &'static str) -> String {
    if path.contains("/__aura_soft_404_probe/") {
        return html_response(200, "OK", HOMEPAGE_HTML);
    }

    if path.contains("itm999999") {
        return html_response(404, "Not Found", "not found");
    }

    if path.contains("/itm284170/aura-soft-404-") {
        return html_response(200, "OK", CANONICAL_PRODUCT_HTML);
    }

    if path.contains("itm284170-aura-soft-404-") {
        return html_response(200, "OK", HOMEPAGE_HTML);
    }

    html_response(200, "OK", html)
}

fn html_response(status: u16, reason: &str, html: &str) -> String {
    format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        html.len(),
        html
    )
}

async fn insert_shop_with_product_url(
    pool: &sqlx::PgPool,
    product_url: &Url,
) -> (ShopId, uuid::Uuid) {
    let shop_id = ShopId::new();
    let shop_id_uuid = uuid::Uuid::from(shop_id);

    sqlx::query(
        "INSERT INTO shops (shop_id, shop_name, shop_type, active, created, updated)
         VALUES ($1, 'Antiques Boutique', 'MARKETPLACE', TRUE, NOW(), NOW())",
    )
    .bind(shop_id_uuid)
    .execute(pool)
    .await
    .unwrap();

    let domain = product_url.host_str().unwrap();
    let row: (uuid::Uuid,) = sqlx::query_as(
        "INSERT INTO shop_domains (shop_id, shop_domain)
         VALUES ($1, $2)
         RETURNING domain_id",
    )
    .bind(shop_id_uuid)
    .bind(domain)
    .fetch_one(pool)
    .await
    .unwrap();

    let repository = UrlMetadataRepositoryImpl::new(pool.clone());
    repository
        .upsert_link(&shop_id, &row.0, product_url, &UrlClass::Product)
        .await
        .unwrap();

    (shop_id, row.0)
}

#[serial]
#[localstack_test(services = [RDS])]
async fn soft_404_should_skip_hard_404_probe_and_store_product_shaped_probe() {
    let html = include_str!("fixtures/html/antiquesboutique_removed.html");
    let (product_url, requested_paths) = spawn_soft_404_server(html).await;

    let pool = get_postgres_client().await;
    let (shop_id, domain_id) = insert_shop_with_product_url(&pool, &product_url).await;
    let candidate_service = Arc::new(ScraperCandidateServiceImpl::new(pool.clone()));

    let scraper = ScraperServiceImpl::new(
        Box::new(ReqwestHtmlFetcher::new()),
        Box::new(PanicSchemaService),
        Box::new(PanicNormalizationService),
        candidate_service.clone(),
    );

    let err = scraper
        .scrape(&shop_id, &product_url, None, None)
        .await
        .unwrap_err();

    assert!(matches!(err, ScraperError::ProductRemoved { .. }));

    let (state,): (String,) =
        sqlx::query_as("SELECT last_scraped_state FROM shop_urls WHERE url = $1")
            .bind(product_url.to_string())
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(state, "REMOVED");

    let (fingerprint, probe_url): (Option<String>, Option<String>) = sqlx::query_as(
        "SELECT soft_404_fingerprint, soft_404_probe_url
         FROM shop_domains
         WHERE domain_id = $1",
    )
    .bind(domain_id)
    .fetch_one(&pool)
    .await
    .unwrap();

    let fingerprint = fingerprint.expect("soft-404 fingerprint should be stored");
    let probe_url = probe_url.expect("soft-404 probe URL should be stored");

    assert!(fingerprint.contains("couldn"));
    assert!(fingerprint.contains("found"));
    assert!(probe_url.contains("/aura-soft-404-"));
    assert!(!probe_url.contains("itm284170"));
    assert!(!probe_url.contains("/__aura_soft_404_probe/"));

    let requested_paths = requested_paths.lock().unwrap();
    assert!(
        !requested_paths
            .iter()
            .any(|path| path.contains("/itm284170/aura-soft-404-")),
        "child probe should not be requested: {requested_paths:?}"
    );
}
