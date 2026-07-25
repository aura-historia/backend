use std::future::Future;
use std::net::{AddrParseError, SocketAddr};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tracing::{debug, error, info};

pub const API_BIND_ADDR_ENV: &str = "AURA_HISTORIA_API_BIND_ADDR";
const DEFAULT_API_BIND_ADDR: &str = "0.0.0.0:8080";
const REQUEST_BUFFER_BYTES: usize = 8192;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApiConfig {
    bind_addr: SocketAddr,
}

impl ApiConfig {
    pub fn from_env() -> Result<Self, ApiConfigError> {
        Self::from_getter(|name| std::env::var(name).ok())
    }

    pub fn from_getter<F>(mut get: F) -> Result<Self, ApiConfigError>
    where
        F: FnMut(&'static str) -> Option<String>,
    {
        let raw_bind_addr =
            get(API_BIND_ADDR_ENV).unwrap_or_else(|| DEFAULT_API_BIND_ADDR.to_owned());
        let bind_addr =
            raw_bind_addr
                .parse()
                .map_err(|source| ApiConfigError::InvalidBindAddr {
                    value: raw_bind_addr,
                    source,
                })?;

        Ok(Self { bind_addr })
    }

    pub const fn bind_addr(&self) -> SocketAddr {
        self.bind_addr
    }
}

#[derive(thiserror::Error, Debug)]
pub enum ApiConfigError {
    #[error("invalid {env_name}: {value}", env_name = API_BIND_ADDR_ENV)]
    InvalidBindAddr {
        value: String,
        source: AddrParseError,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HealthResponse {
    pub status_code: u16,
    pub body: &'static str,
}

pub fn route(method: &str, path: &str) -> HealthResponse {
    match (method, path) {
        ("GET", "/health") => HealthResponse {
            status_code: 200,
            body: "ok\n",
        },
        ("GET", "/ready") => HealthResponse {
            status_code: 200,
            body: "ready\n",
        },
        _ => HealthResponse {
            status_code: 404,
            body: "not found\n",
        },
    }
}

pub async fn run_until_shutdown<S>(config: ApiConfig, shutdown: S) -> Result<(), ApiRunError>
where
    S: Future<Output = ()>,
{
    let listener = TcpListener::bind(config.bind_addr())
        .await
        .map_err(ApiRunError::Bind)?;
    serve(listener, shutdown).await
}

pub async fn serve<S>(listener: TcpListener, shutdown: S) -> Result<(), ApiRunError>
where
    S: Future<Output = ()>,
{
    let local_addr = listener.local_addr().map_err(ApiRunError::LocalAddr)?;
    info!(bind_addr = %local_addr, "aura-historia-api listening");
    tokio::pin!(shutdown);

    loop {
        tokio::select! {
            accept_result = listener.accept() => {
                let (stream, peer_addr) = accept_result.map_err(ApiRunError::Accept)?;
                debug!(%peer_addr, "accepted API connection");
                tokio::spawn(async move {
                    if let Err(error) = handle_connection(stream).await {
                        error!(%error, "API connection failed");
                    }
                });
            }
            () = &mut shutdown => {
                info!("aura-historia-api shutdown requested");
                return Ok(());
            }
        }
    }
}

async fn handle_connection(mut stream: TcpStream) -> Result<(), std::io::Error> {
    let mut buffer = [0_u8; REQUEST_BUFFER_BYTES];
    let bytes_read = stream.read(&mut buffer).await?;
    let request = String::from_utf8_lossy(&buffer[..bytes_read]);
    let (method, path) = parse_request_line(&request).unwrap_or(("", ""));
    let response = route(method, path);
    write_response(&mut stream, response).await
}

fn parse_request_line(request: &str) -> Option<(&str, &str)> {
    let line = request.lines().next()?;
    let mut parts = line.split_whitespace();
    let method = parts.next()?;
    let path = parts.next()?;
    Some((method, path))
}

async fn write_response(
    stream: &mut TcpStream,
    response: HealthResponse,
) -> Result<(), std::io::Error> {
    let status = match response.status_code {
        200 => "200 OK",
        404 => "404 Not Found",
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
pub enum ApiRunError {
    #[error("failed to bind API listener")]
    Bind(#[source] std::io::Error),
    #[error("failed to read API listener local address")]
    LocalAddr(#[source] std::io::Error),
    #[error("failed to accept API connection")]
    Accept(#[source] std::io::Error),
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;
    use rstest::rstest;
    use tokio::sync::oneshot;

    fn env(values: &[(&'static str, &str)]) -> HashMap<&'static str, String> {
        values
            .iter()
            .map(|(key, value)| (*key, (*value).to_owned()))
            .collect()
    }

    #[test]
    fn should_use_default_bind_addr_when_env_missing() -> Result<(), Box<dyn std::error::Error>> {
        let values = env(&[]);

        let config = ApiConfig::from_getter(|name| values.get(name).cloned())?;

        assert_eq!("0.0.0.0:8080".parse::<SocketAddr>()?, config.bind_addr());
        Ok(())
    }

    #[test]
    fn should_read_bind_addr_from_env() -> Result<(), Box<dyn std::error::Error>> {
        let values = env(&[(API_BIND_ADDR_ENV, "127.0.0.1:9000")]);

        let config = ApiConfig::from_getter(|name| values.get(name).cloned())?;

        assert_eq!("127.0.0.1:9000".parse::<SocketAddr>()?, config.bind_addr());
        Ok(())
    }

    #[test]
    fn should_fail_when_bind_addr_is_invalid() {
        let values = env(&[(API_BIND_ADDR_ENV, "not-an-addr")]);

        let config = ApiConfig::from_getter(|name| values.get(name).cloned());

        assert!(matches!(
            config,
            Err(ApiConfigError::InvalidBindAddr { .. })
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
    async fn should_serve_health_endpoint_until_shutdown() -> Result<(), Box<dyn std::error::Error>>
    {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let addr = listener.local_addr()?;
        let (shutdown_tx, shutdown_rx) = oneshot::channel();
        let server = tokio::spawn(serve(listener, async move {
            let _ = shutdown_rx.await;
        }));

        let mut stream = TcpStream::connect(addr).await?;
        stream
            .write_all(b"GET /health HTTP/1.1\r\nhost: localhost\r\n\r\n")
            .await?;
        let mut response = String::new();
        stream.read_to_string(&mut response).await?;
        let _send_result = shutdown_tx.send(());
        server.await??;

        assert!(response.starts_with("HTTP/1.1 200 OK"));
        assert!(response.ends_with("ok\n"));
        Ok(())
    }
}
