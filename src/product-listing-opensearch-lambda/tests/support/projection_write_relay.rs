//! Test-only relay. It forwards one real projection request to OpenSearch unchanged, then either
//! holds its response for a newer target write or drops it after target acceptance.
use opensearch::{
    OpenSearch,
    http::{Method, StatusCode, Url, headers::HeaderMap, request::JsonBody, transport::Transport},
};
use std::{io, time::Duration};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
    sync::oneshot,
    task::JoinHandle,
    time::timeout,
};

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

enum ResponseMode {
    Pause(oneshot::Receiver<()>),
    Drop,
}

pub(super) struct ProjectionWriteRelay {
    pub(super) client: OpenSearch,
    received: oneshot::Receiver<()>,
    release: Option<oneshot::Sender<()>>,
    forwarded: JoinHandle<TestResult<StatusCode>>,
}

impl ProjectionWriteRelay {
    pub(super) async fn pause(target: OpenSearch) -> TestResult<Self> {
        Self::new(target, true).await
    }

    pub(super) async fn drop_response(target: OpenSearch) -> TestResult<Self> {
        Self::new(target, false).await
    }

    async fn new(target: OpenSearch, pause: bool) -> TestResult<Self> {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let client = OpenSearch::new(Transport::single_node(&format!(
            "http://{}",
            listener.local_addr()?
        ))?);
        let (received_tx, received) = oneshot::channel();
        let (release, mode) = if pause {
            let (release_tx, release_rx) = oneshot::channel();
            (Some(release_tx), ResponseMode::Pause(release_rx))
        } else {
            (None, ResponseMode::Drop)
        };
        let forwarded = tokio::spawn(async move {
            timeout(Duration::from_secs(30), async move {
                let (mut socket, _) = listener.accept().await?;
                let request = read_request(&mut socket).await?;
                received_tx
                    .send(())
                    .map_err(|_| io::Error::other("test receiver dropped"))?;
                if let ResponseMode::Pause(release) = mode {
                    release
                        .await
                        .map_err(|_| io::Error::other("test relay release dropped"))?;
                }
                let response = target
                    .send(
                        request.method,
                        request.url.path(),
                        HeaderMap::new(),
                        Some(&request.query),
                        request.body,
                        None,
                    )
                    .await?;
                let status = response.status_code();
                let body = response.text().await?;
                if pause {
                    let reply = format!(
                        "HTTP/1.1 {} Test\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        status.as_u16(),
                        body.len(),
                        body
                    );
                    socket.write_all(reply.as_bytes()).await?;
                    socket.shutdown().await?;
                }
                Ok(status)
            })
            .await?
        });
        Ok(Self {
            client,
            received,
            release,
            forwarded,
        })
    }

    pub(super) async fn wait_until_received(&mut self) -> TestResult {
        timeout(Duration::from_secs(10), &mut self.received)
            .await
            .map_err(|_| io::Error::other("timed out waiting for projection write"))?
            .map_err(|_| io::Error::other("projection writer dropped before target request"))?;
        Ok(())
    }

    pub(super) async fn resume(mut self) -> TestResult<u16> {
        let release = self
            .release
            .take()
            .ok_or_else(|| io::Error::other("response-loss relay cannot resume"))?;
        release
            .send(())
            .map_err(|_| io::Error::other("test relay release dropped"))?;
        Ok(timeout(Duration::from_secs(10), self.forwarded)
            .await???
            .as_u16())
    }

    pub(super) async fn finish(self) -> TestResult<u16> {
        Ok(timeout(Duration::from_secs(10), self.forwarded)
            .await???
            .as_u16())
    }
}

struct ProjectionRequest {
    method: Method,
    url: Url,
    query: Vec<(String, String)>,
    body: Option<JsonBody<serde_json::Value>>,
}

async fn read_request(socket: &mut tokio::net::TcpStream) -> TestResult<ProjectionRequest> {
    let mut bytes = Vec::new();
    let (header_end, content_length) = loop {
        let count = socket.read_buf(&mut bytes).await?;
        if count == 0 || bytes.len() > 1_000_000 {
            return Err(io::Error::other("incomplete or oversized projection test request").into());
        }
        if let Some(end) = bytes.windows(4).position(|part| part == b"\r\n\r\n") {
            let headers = std::str::from_utf8(&bytes[..end])?;
            let content_length = headers
                .lines()
                .find_map(|line| {
                    let (name, value) = line.split_once(':')?;
                    name.eq_ignore_ascii_case("content-length")
                        .then_some(value.trim())
                })
                .unwrap_or("0")
                .parse::<usize>()?;
            break (end + 4, content_length);
        }
    };
    while bytes.len() < header_end + content_length {
        if socket.read_buf(&mut bytes).await? == 0 {
            return Err(io::Error::other("incomplete projection test request body").into());
        }
    }
    let headers = std::str::from_utf8(&bytes[..header_end])?;
    let mut request_line = headers
        .lines()
        .next()
        .ok_or_else(|| io::Error::other("missing projection request line"))?
        .split_whitespace();
    let method = match request_line.next() {
        Some("PUT") => Method::Put,
        Some("POST") => Method::Post,
        Some("DELETE") => Method::Delete,
        _ => return Err(io::Error::other("expected a projection write request").into()),
    };
    let path = request_line
        .next()
        .ok_or_else(|| io::Error::other("missing projection request path"))?;
    let url = Url::parse(&format!("http://relay.test{path}"))?;
    let query = url.query_pairs().into_owned().collect();
    let body = (content_length != 0)
        .then(|| serde_json::from_slice(&bytes[header_end..header_end + content_length]))
        .transpose()?
        .map(JsonBody::new);
    Ok(ProjectionRequest {
        method,
        url,
        query,
        body,
    })
}
