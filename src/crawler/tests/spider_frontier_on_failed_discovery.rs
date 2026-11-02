//! Two consecutive spider runs on a shop whose pages lie deeper than one crawl budget, both
//! failing because their pages hold no product URL: the real spider crawls a local site, the
//! crawler's PostgreSQL repositories keep its frontier, and the second run reaches further
//! (`docs/crawler/scheduling-and-retries.md#discovery-coverage`).

use async_trait::async_trait;
use crawler::CrawlerDomainId;
use crawler::review::repository::CrawlerReviewRepository;
use crawler::spider::classification::url_metadata_repository::UrlMetadataRepositoryImpl;
use crawler::spider::classification::url_pattern_repository::ListingSourceUrlPatternRepositoryImpl;
use crawler::spider::classification::url_pattern_service::{
    UrlPatternAuthoring, UrlPatternServiceImpl,
};
use crawler::spider::crawl_frontier_repository::{
    CrawlFrontierRepository, CrawlFrontierRepositoryImpl,
};
use crawler::spider::discovery::frontier::CrawlKind;
use crawler::spider::discovery::website_spider::{
    CrawlPace, CrawlerConfig, SpiderImpl, SpiderTransport, SpiderTransportError,
};
use crawler::spider::service::{
    SpiderService, SpiderServiceConfig, SpiderServiceError, SpiderServiceImpl,
};
use listing_source_core::ListingSourceId;
use sqlx::PgPool;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex, Once};
use std::time::Duration;
use test_api::*;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use url::Url;

const POSTGRES: Postgres = Postgres::new("src/crawler/migrations");
const HOST: &str = "deep-shop.example.com";
/// Category pages chained one below the other, far more than one crawl reaches.
const DEPTH: usize = 60;
/// How long the shop takes to answer a category page.
const PAGE_LATENCY: Duration = Duration::from_millis(250);
/// One crawl's wall-clock budget: a dozen pages at the shop's latency, below the inference
/// sample. A seeded crawl walks from the root for half of it, then turns to its frontier.
const CRAWL_BUDGET: Duration = Duration::from_secs(3);

/// The real spider reads its page-body ceiling from the environment, as the server does.
fn configure_spider_body_limit() {
    static CONFIGURED: Once = Once::new();
    CONFIGURED.call_once(|| {
        // SAFETY: set once, before this test binary's first crawl reads it, and nothing in the
        // binary writes the environment concurrently.
        unsafe { std::env::set_var("SPIDER_MAX_SIZE_BYTES", "8388608") };
    });
}

/// A shop whose root links the first of [`DEPTH`] chained category pages, none of them a
/// product page. Records the path of every page request.
struct DeepShop {
    addr: SocketAddr,
    requested: Arc<Mutex<Vec<String>>>,
    server: tokio::task::JoinHandle<()>,
}

impl DeepShop {
    async fn spawn() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let requested = Arc::new(Mutex::new(Vec::new()));
        let log = requested.clone();
        let server = tokio::spawn(async move {
            while let Ok((mut stream, _)) = listener.accept().await {
                let log = log.clone();
                tokio::spawn(async move {
                    let mut request = [0; 4096];
                    let Ok(read) = stream.read(&mut request).await else {
                        return;
                    };
                    let request = String::from_utf8_lossy(&request[..read]);
                    let path = request
                        .split_whitespace()
                        .nth(1)
                        .unwrap_or_default()
                        .to_string();
                    let (status, body) = Self::page(&path);
                    if path.starts_with("/category/") {
                        tokio::time::sleep(PAGE_LATENCY).await;
                    }
                    log.lock().unwrap().push(path);
                    let response = format!(
                        "HTTP/1.1 {status}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    );
                    let _ = stream.write_all(response.as_bytes()).await;
                });
            }
        });
        Self {
            addr,
            requested,
            server,
        }
    }

    fn page(path: &str) -> (&'static str, String) {
        let category = |depth: usize| {
            let next = if depth < DEPTH {
                format!(r#"<a href="/category/{}">Next category</a>"#, depth + 1)
            } else {
                String::new()
            };
            (
                "200 OK",
                format!("<!doctype html><title>Category {depth}</title><p>Antiques</p>{next}"),
            )
        };
        match path {
            "/" => category(0),
            _ => match path
                .strip_prefix("/category/")
                .and_then(|depth| depth.parse::<usize>().ok())
            {
                Some(depth) if (1..=DEPTH).contains(&depth) => category(depth),
                _ => ("404 Not Found", String::new()),
            },
        }
    }

    /// The deepest category page requested since the last call, and forgets the requests.
    fn take_deepest_category(&self) -> usize {
        std::mem::take(&mut *self.requested.lock().unwrap())
            .iter()
            .filter_map(|path| path.strip_prefix("/category/")?.parse().ok())
            .max()
            .unwrap_or(0)
    }

    fn transport(&self) -> Arc<dyn SpiderTransport> {
        Arc::new(PinnedTransport { addr: self.addr })
    }
}

impl Drop for DeepShop {
    fn drop(&mut self) {
        self.server.abort();
    }
}

/// Resolves the shop's host to the local listener, the way the production transport pins
/// public addresses.
struct PinnedTransport {
    addr: SocketAddr,
}

#[async_trait]
impl SpiderTransport for PinnedTransport {
    async fn client_for(
        &self,
        _url: &Url,
        timeout: Duration,
    ) -> Result<spider::reqwest::Client, SpiderTransportError> {
        spider::reqwest::Client::builder()
            .resolve(HOST, self.addr)
            .redirect(spider::reqwest::redirect::Policy::none())
            .timeout(timeout)
            .no_proxy()
            .build()
            .map_err(SpiderTransportError::Client)
    }
}

async fn register_domain(pool: &PgPool) -> (ListingSourceId, CrawlerDomainId) {
    let listing_source_id = ListingSourceId::new();
    sqlx::query(
        "INSERT INTO listing_sources (listing_source_id, listing_source_name, listing_source_slug, crawl_enabled) \
         VALUES ($1, 'Deep shop', 'deep-shop', TRUE)",
    )
    .bind(listing_source_id.as_uuid())
    .execute(pool)
    .await
    .unwrap();
    let domain_id = CrawlerDomainId::new();
    sqlx::query(
        "INSERT INTO listing_source_domains (domain_id, listing_source_id, listing_source_domain, crawl_root_host) \
         VALUES ($1, $2, $3, $3)",
    )
    .bind(domain_id.as_uuid())
    .bind(listing_source_id.as_uuid())
    .bind(HOST)
    .execute(pool)
    .await
    .unwrap();
    (listing_source_id, domain_id)
}

fn spider_service(pool: &PgPool, shop: &DeepShop) -> SpiderServiceImpl {
    let spider = SpiderImpl::with_transport(
        CrawlerConfig {
            delay_millis: 0,
            request_timeout_secs: 5,
            concurrency_limit: 1,
            channel_size: 32,
            max_crawl_duration: CRAWL_BUDGET,
            ..CrawlerConfig::default()
        },
        shop.transport(),
    );
    SpiderServiceImpl::new(
        SpiderServiceConfig::default(),
        Box::new(spider),
        Box::new(UrlPatternServiceImpl::new(
            Arc::new(ListingSourceUrlPatternRepositoryImpl::new(pool.clone())),
            UrlPatternAuthoring::Manual {
                review_repository: CrawlerReviewRepository::new(pool.clone()),
            },
        )),
        Arc::new(UrlMetadataRepositoryImpl::new(pool.clone())),
        Arc::new(CrawlFrontierRepositoryImpl::new(pool.clone())),
    )
}

#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn failing_runs_on_a_deep_shop_should_reach_further_each_time() {
    configure_spider_body_limit();
    let pool = get_postgres_client().await;
    let (listing_source_id, domain_id) = register_domain(&pool).await;
    let shop = DeepShop::spawn().await;
    let service = spider_service(&pool, &shop);
    let frontier = CrawlFrontierRepositoryImpl::new(pool.clone());
    let crawl_root_url = format!("http://{HOST}/");
    let crawl = || {
        service.run(
            &listing_source_id,
            &domain_id,
            &crawl_root_url,
            400,
            CrawlPace::default(),
            CrawlKind::Full,
        )
    };

    let first = crawl()
        .await
        .expect_err("too few pages for an inference sample");
    let first_deepest = shop.take_deepest_category();
    let first_frontier = frontier.load_frontier(&domain_id).await.unwrap();

    assert!(
        matches!(
            first,
            SpiderServiceError::InsufficientInferenceSample { .. }
        ),
        "{first:?}"
    );
    assert!(
        (1..DEPTH).contains(&first_deepest),
        "one crawl reaches part of the shop: {first_deepest}"
    );
    assert!(
        !first_frontier.is_empty(),
        "the failed run that delivered pages keeps its frontier"
    );

    let second = crawl()
        .await
        .expect_err("too few pages for an inference sample");
    let second_deepest = shop.take_deepest_category();

    assert!(
        matches!(
            second,
            SpiderServiceError::InsufficientInferenceSample { .. }
        ),
        "{second:?}"
    );
    assert!(
        second_deepest > first_deepest,
        "the second run continues from the first run's frontier: reached {second_deepest} after {first_deepest}"
    );
    assert_ne!(
        frontier.load_frontier(&domain_id).await.unwrap(),
        first_frontier,
        "the second run replaced the frontier with its own"
    );
}
