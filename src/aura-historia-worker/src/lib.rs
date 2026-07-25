pub mod cdc;
pub mod retry;

use std::future::Future;
use std::net::{AddrParseError, SocketAddr};
use std::sync::Arc;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{Mutex, mpsc};
use tracing::{debug, error, info};

use crate::cdc::{CdcFanout, CdcIngestError, WorkerQueueReceivers, WorkerQueueRegistry};

pub const WORKER_HEALTH_BIND_ADDR_ENV: &str = "AURA_HISTORIA_WORKER_HEALTH_BIND_ADDR";
const DEFAULT_WORKER_HEALTH_BIND_ADDR: &str = "0.0.0.0:8081";
const REQUEST_BUFFER_BYTES: usize = 65_536;
pub const SEQUIN_CDC_PATH: &str = "/cdc/sequin";

pub trait WorkerJob: Send + 'static {}

impl<T> WorkerJob for T where T: Send + 'static {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkerConfig {
    health_bind_addr: SocketAddr,
}

impl WorkerConfig {
    pub fn from_env() -> Result<Self, WorkerConfigError> {
        Self::from_getter(|name| std::env::var(name).ok())
    }

    pub fn from_getter<F>(mut get: F) -> Result<Self, WorkerConfigError>
    where
        F: FnMut(&'static str) -> Option<String>,
    {
        let raw_health_bind_addr = get(WORKER_HEALTH_BIND_ADDR_ENV)
            .unwrap_or_else(|| DEFAULT_WORKER_HEALTH_BIND_ADDR.to_owned());
        let health_bind_addr = raw_health_bind_addr.parse().map_err(|source| {
            WorkerConfigError::InvalidHealthBindAddr {
                value: raw_health_bind_addr,
                source,
            }
        })?;

        Ok(Self { health_bind_addr })
    }

    pub const fn health_bind_addr(&self) -> SocketAddr {
        self.health_bind_addr
    }
}

#[derive(thiserror::Error, Debug)]
pub enum WorkerConfigError {
    #[error("invalid {env_name}: {value}", env_name = WORKER_HEALTH_BIND_ADDR_ENV)]
    InvalidHealthBindAddr {
        value: String,
        source: AddrParseError,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct QueueConfig {
    capacity: usize,
}

impl QueueConfig {
    pub const fn new(capacity: usize) -> Self {
        Self { capacity }
    }

    pub const fn capacity(&self) -> usize {
        self.capacity
    }
}

#[derive(Debug, Clone)]
pub struct InMemoryQueueSender<T> {
    sender: mpsc::Sender<T>,
}

#[derive(Debug)]
pub struct InMemoryQueueReceiver<T> {
    receiver: mpsc::Receiver<T>,
}

pub fn in_memory_queue<T>(
    config: QueueConfig,
) -> Result<(InMemoryQueueSender<T>, InMemoryQueueReceiver<T>), QueueConfigError>
where
    T: WorkerJob,
{
    if config.capacity() == 0 {
        return Err(QueueConfigError::InvalidCapacity);
    }

    let (sender, receiver) = mpsc::channel(config.capacity());
    Ok((
        InMemoryQueueSender { sender },
        InMemoryQueueReceiver { receiver },
    ))
}

#[derive(thiserror::Error, Debug, PartialEq, Eq)]
pub enum QueueConfigError {
    #[error("queue capacity must be greater than zero")]
    InvalidCapacity,
}

impl<T> InMemoryQueueSender<T>
where
    T: WorkerJob,
{
    pub async fn enqueue(&self, job: T) -> Result<(), mpsc::error::SendError<T>> {
        self.sender.send(job).await
    }

    pub fn try_enqueue(&self, job: T) -> Result<(), mpsc::error::TrySendError<T>> {
        self.sender.try_send(job)
    }
}

impl<T> InMemoryQueueReceiver<T>
where
    T: WorkerJob,
{
    pub async fn recv(&mut self) -> Option<T> {
        self.receiver.recv().await
    }
}

#[derive(Debug, Clone)]
pub struct WorkerRuntime {
    cdc_fanout: CdcFanout,
    _default_receivers: Option<Arc<Mutex<WorkerQueueReceivers>>>,
}

impl WorkerRuntime {
    pub fn new(cdc_fanout: CdcFanout) -> Self {
        Self {
            cdc_fanout,
            _default_receivers: None,
        }
    }

    pub fn with_all_queues(
        config: QueueConfig,
    ) -> Result<(Self, WorkerQueueReceivers), QueueConfigError> {
        let (registry, receivers) = WorkerQueueRegistry::with_all_queues(config)?;
        Ok((Self::new(CdcFanout::new(registry)), receivers))
    }

    pub fn empty() -> Self {
        Self::new(CdcFanout::new(WorkerQueueRegistry::new()))
    }

    pub async fn ingest_cdc_json(&self, body: &str) -> Result<usize, CdcIngestError> {
        self.cdc_fanout.ingest_json(body).await
    }
}

impl Default for WorkerRuntime {
    fn default() -> Self {
        let (runtime, receivers) = Self::with_all_queues(QueueConfig::new(1024))
            .expect("default queue capacity should be valid");
        Self {
            cdc_fanout: runtime.cdc_fanout,
            _default_receivers: Some(Arc::new(Mutex::new(receivers))),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpResponse {
    pub status_code: u16,
    pub body: &'static str,
}

pub fn route(method: &str, path: &str) -> HttpResponse {
    match (method, path) {
        ("GET", "/health") => HttpResponse {
            status_code: 200,
            body: "ok\n",
        },
        ("GET", "/ready") => HttpResponse {
            status_code: 200,
            body: "ready\n",
        },
        _ => HttpResponse {
            status_code: 404,
            body: "not found\n",
        },
    }
}

pub async fn run_until_shutdown<S>(config: WorkerConfig, shutdown: S) -> Result<(), WorkerRunError>
where
    S: Future<Output = ()>,
{
    run_until_shutdown_with_runtime(config, WorkerRuntime::default(), shutdown).await
}

pub async fn run_until_shutdown_with_runtime<S>(
    config: WorkerConfig,
    runtime: WorkerRuntime,
    shutdown: S,
) -> Result<(), WorkerRunError>
where
    S: Future<Output = ()>,
{
    let listener = TcpListener::bind(config.health_bind_addr())
        .await
        .map_err(WorkerRunError::Bind)?;
    serve_with_runtime(listener, runtime, shutdown).await
}

pub async fn serve<S>(listener: TcpListener, shutdown: S) -> Result<(), WorkerRunError>
where
    S: Future<Output = ()>,
{
    serve_with_runtime(listener, WorkerRuntime::default(), shutdown).await
}

pub async fn serve_with_runtime<S>(
    listener: TcpListener,
    runtime: WorkerRuntime,
    shutdown: S,
) -> Result<(), WorkerRunError>
where
    S: Future<Output = ()>,
{
    let local_addr = listener.local_addr().map_err(WorkerRunError::LocalAddr)?;
    info!(bind_addr = %local_addr, "aura-historia-worker health and CDC server listening");
    tokio::pin!(shutdown);
    let runtime = Arc::new(runtime);

    loop {
        tokio::select! {
            accept_result = listener.accept() => {
                let (stream, peer_addr) = accept_result.map_err(WorkerRunError::Accept)?;
                let runtime = Arc::clone(&runtime);
                debug!(%peer_addr, "accepted worker connection");
                tokio::spawn(async move {
                    if let Err(error) = handle_connection(stream, runtime).await {
                        error!(%error, "worker connection failed");
                    }
                });
            }
            () = &mut shutdown => {
                info!("aura-historia-worker shutdown requested");
                return Ok(());
            }
        }
    }
}

async fn handle_connection(
    mut stream: TcpStream,
    runtime: Arc<WorkerRuntime>,
) -> Result<(), std::io::Error> {
    let mut buffer = [0_u8; REQUEST_BUFFER_BYTES];
    let bytes_read = stream.read(&mut buffer).await?;
    let request = String::from_utf8_lossy(&buffer[..bytes_read]);
    let parsed_request = parse_http_request(&request);
    let response = handle_request(parsed_request, runtime).await;
    write_response(&mut stream, response).await
}

async fn handle_request(
    parsed_request: Option<HttpRequest<'_>>,
    runtime: Arc<WorkerRuntime>,
) -> HttpResponse {
    let Some(request) = parsed_request else {
        return HttpResponse {
            status_code: 400,
            body: "bad request\n",
        };
    };

    if request.method == "POST" && request.path == SEQUIN_CDC_PATH {
        return match runtime.ingest_cdc_json(request.body).await {
            Ok(_) => HttpResponse {
                status_code: 202,
                body: "accepted\n",
            },
            Err(CdcIngestError::InvalidJson(_)) => HttpResponse {
                status_code: 400,
                body: "invalid CDC JSON\n",
            },
            Err(error) => {
                error!(%error, "CDC fanout failed; requesting Sequin retry");
                HttpResponse {
                    status_code: 503,
                    body: "CDC fanout failed\n",
                }
            }
        };
    }

    route(request.method, request.path)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct HttpRequest<'a> {
    method: &'a str,
    path: &'a str,
    body: &'a str,
}

fn parse_http_request(request: &str) -> Option<HttpRequest<'_>> {
    let (head, body) = request.split_once("\r\n\r\n").unwrap_or((request, ""));
    let line = head.lines().next()?;
    let mut parts = line.split_whitespace();
    let method = parts.next()?;
    let path = parts.next()?;
    Some(HttpRequest { method, path, body })
}

async fn write_response(
    stream: &mut TcpStream,
    response: HttpResponse,
) -> Result<(), std::io::Error> {
    let status = match response.status_code {
        200 => "200 OK",
        202 => "202 Accepted",
        400 => "400 Bad Request",
        404 => "404 Not Found",
        503 => "503 Service Unavailable",
        _ => "500 Internal Server Error",
    };
    let bytes = response.body.as_bytes();
    stream
        .write_all(
            format!(
                "HTTP/1.1 {status}\r\ncontent-length: {}\r\ncontent-type: text/plain\r\nconnection: close\r\n\r\n{}",
                bytes.len(),
                response.body
            )
            .as_bytes(),
        )
        .await
}

#[derive(thiserror::Error, Debug)]
pub enum WorkerRunError {
    #[error("failed to bind worker health listener")]
    Bind(#[source] std::io::Error),
    #[error("failed to read worker health listener local address")]
    LocalAddr(#[source] std::io::Error),
    #[error("failed to accept worker health connection")]
    Accept(#[source] std::io::Error),
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::net::SocketAddr;

    use super::*;
    use crate::cdc::{CdcFanout, DomainJob, WorkerQueue, WorkerQueueRegistry};
    use rstest::rstest;
    use tokio::sync::oneshot;

    fn env(values: &[(&'static str, &str)]) -> HashMap<&'static str, String> {
        values
            .iter()
            .map(|(key, value)| (*key, (*value).to_owned()))
            .collect()
    }

    #[test]
    fn should_use_default_health_bind_addr_when_env_missing()
    -> Result<(), Box<dyn std::error::Error>> {
        let values = env(&[]);

        let config = WorkerConfig::from_getter(|name| values.get(name).cloned())?;

        assert_eq!(
            "0.0.0.0:8081".parse::<SocketAddr>()?,
            config.health_bind_addr()
        );
        Ok(())
    }

    #[test]
    fn should_read_health_bind_addr_from_env() -> Result<(), Box<dyn std::error::Error>> {
        let values = env(&[(WORKER_HEALTH_BIND_ADDR_ENV, "127.0.0.1:9001")]);

        let config = WorkerConfig::from_getter(|name| values.get(name).cloned())?;

        assert_eq!(
            "127.0.0.1:9001".parse::<SocketAddr>()?,
            config.health_bind_addr()
        );
        Ok(())
    }

    #[test]
    fn should_fail_when_health_bind_addr_is_invalid() {
        let values = env(&[(WORKER_HEALTH_BIND_ADDR_ENV, "not-an-addr")]);

        let config = WorkerConfig::from_getter(|name| values.get(name).cloned());

        assert!(matches!(
            config,
            Err(WorkerConfigError::InvalidHealthBindAddr { .. })
        ));
    }

    #[rstest]
    #[case("GET", "/health", 200, "ok\n")]
    #[case("GET", "/ready", 200, "ready\n")]
    #[case("POST", "/health", 404, "not found\n")]
    fn should_route_health_endpoints(
        #[case] method: &str,
        #[case] path: &str,
        #[case] status_code: u16,
        #[case] body: &'static str,
    ) {
        let response = route(method, path);

        assert_eq!(status_code, response.status_code);
        assert_eq!(body, response.body);
    }

    #[tokio::test]
    async fn should_enqueue_and_receive_jobs() -> Result<(), Box<dyn std::error::Error>> {
        let (sender, mut receiver) = in_memory_queue::<String>(QueueConfig::new(2))?;

        sender.enqueue("product:1".to_owned()).await?;

        assert_eq!(Some("product:1".to_owned()), receiver.recv().await);
        Ok(())
    }

    #[tokio::test]
    async fn should_apply_backpressure_when_queue_is_full() -> Result<(), Box<dyn std::error::Error>>
    {
        let (sender, _receiver) = in_memory_queue::<String>(QueueConfig::new(1))?;

        let first_result = sender.try_enqueue("product:1".to_owned());
        let second_result = sender.try_enqueue("product:2".to_owned());

        assert!(first_result.is_ok());
        assert!(matches!(
            second_result,
            Err(mpsc::error::TrySendError::Full(_))
        ));
        Ok(())
    }

    #[test]
    fn should_reject_zero_queue_capacity() {
        let queue = in_memory_queue::<String>(QueueConfig::new(0));

        assert!(matches!(queue, Err(QueueConfigError::InvalidCapacity)));
    }

    #[tokio::test]
    async fn should_serve_health_endpoint_until_shutdown() -> Result<(), Box<dyn std::error::Error>>
    {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let addr = listener.local_addr()?;
        let (shutdown_tx, shutdown_rx) = oneshot::channel();
        let server = tokio::spawn(serve(listener, async move {
            let _ = shutdown_rx.await;
        }));

        let response = request(addr, "GET /health HTTP/1.1\r\nhost: localhost\r\n\r\n").await?;
        let _send_result = shutdown_tx.send(());
        server.await??;

        assert!(response.starts_with("HTTP/1.1 200 OK"));
        assert!(response.ends_with("ok\n"));
        Ok(())
    }

    #[tokio::test]
    async fn should_accept_sequin_cdc_after_fanout() -> Result<(), Box<dyn std::error::Error>> {
        let (product_sender, mut product_receiver) =
            in_memory_queue::<DomainJob>(QueueConfig::new(8))?;
        let (percolator_sender, mut percolator_receiver) =
            in_memory_queue::<DomainJob>(QueueConfig::new(8))?;
        let (embed_sender, mut embed_receiver) = in_memory_queue::<DomainJob>(QueueConfig::new(8))?;
        let runtime = WorkerRuntime::new(CdcFanout::new(
            WorkerQueueRegistry::new()
                .with_queue(WorkerQueue::ProductOpenSearch, product_sender)
                .with_queue(WorkerQueue::SearchFilterPercolator, percolator_sender)
                .with_queue(WorkerQueue::ProductEmbed, embed_sender),
        ));
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let addr = listener.local_addr()?;
        let (shutdown_tx, shutdown_rx) = oneshot::channel();
        let server = tokio::spawn(serve_with_runtime(listener, runtime, async move {
            let _ = shutdown_rx.await;
        }));
        let body = r#"{
            "changes": [
                {
                    "table": "product_events",
                    "operation": "insert",
                    "record": {
                        "event_id": "40000000-0000-0000-0000-000000000001",
                        "product_id": "30000000-0000-0000-0000-000000000001",
                        "event_type": "DOMAIN_CREATED",
                        "event_group": "DOMAIN"
                    }
                }
            ]
        }"#;
        let request_text = format!(
            "POST {SEQUIN_CDC_PATH} HTTP/1.1\r\nhost: localhost\r\ncontent-length: {}\r\n\r\n{}",
            body.len(),
            body
        );

        let response = request(addr, &request_text).await?;
        let _send_result = shutdown_tx.send(());
        server.await??;

        assert!(response.starts_with("HTTP/1.1 202 Accepted"));
        assert!(product_receiver.recv().await.is_some());
        assert!(percolator_receiver.recv().await.is_some());
        assert!(embed_receiver.recv().await.is_some());
        Ok(())
    }

    #[tokio::test]
    async fn should_reject_invalid_sequin_cdc_json() -> Result<(), Box<dyn std::error::Error>> {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let addr = listener.local_addr()?;
        let (shutdown_tx, shutdown_rx) = oneshot::channel();
        let server = tokio::spawn(serve(listener, async move {
            let _ = shutdown_rx.await;
        }));
        let request_text = format!(
            "POST {SEQUIN_CDC_PATH} HTTP/1.1\r\nhost: localhost\r\ncontent-length: 8\r\n\r\nnot-json"
        );

        let response = request(addr, &request_text).await?;
        let _send_result = shutdown_tx.send(());
        server.await??;

        assert!(response.starts_with("HTTP/1.1 400 Bad Request"));
        Ok(())
    }

    async fn request(
        addr: SocketAddr,
        request_text: &str,
    ) -> Result<String, Box<dyn std::error::Error>> {
        let mut stream = TcpStream::connect(addr).await?;
        stream.write_all(request_text.as_bytes()).await?;
        let mut response = String::new();
        stream.read_to_string(&mut response).await?;
        Ok(response)
    }
}
