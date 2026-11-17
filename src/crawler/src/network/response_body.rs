//! Reads the body of a shop's answer within a size limit, through one implementation for every
//! crawler client that reads shop bodies: the scraper's, and the spider's own requests on the
//! reqwest major version Spider re-exports.

use crate::network::policy::ReqwestErrorLike;
use hyper::body::Bytes;
use reqwest::header::{CONTENT_LENGTH, HeaderMap};
use std::future::Future;
use std::io::{self, Write};

/// A response whose body the crawler reads from a shop.
pub trait ShopResponse {
    type Error: ReqwestErrorLike + Send + Sync;

    fn headers(&self) -> &HeaderMap;

    /// The next chunk of the body as it came off the connection; `None` at its end.
    fn chunk(&mut self) -> impl Future<Output = Result<Option<Bytes>, Self::Error>> + Send;
}

impl ShopResponse for reqwest::Response {
    type Error = reqwest::Error;

    fn headers(&self) -> &HeaderMap {
        reqwest::Response::headers(self)
    }

    fn chunk(&mut self) -> impl Future<Output = Result<Option<Bytes>, Self::Error>> + Send {
        reqwest::Response::chunk(self)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ShopBodyError<E: std::error::Error + 'static> {
    #[error("response body could not be read")]
    Transport(#[source] E),
    #[error("response body exceeds {limit} bytes")]
    TooLarge { limit: usize },
}

/// The whole body, failing with [`ShopBodyError::TooLarge`] when it exceeds `limit` bytes,
/// declared or read.
pub async fn read_complete_body<R: ShopResponse>(
    response: R,
    limit: usize,
) -> Result<Vec<u8>, ShopBodyError<R::Error>> {
    read_body(response, limit, Overflow::Refuse).await
}

/// The first `limit` bytes of the body; the rest is not read.
pub async fn read_body_prefix<R: ShopResponse>(
    response: R,
    limit: usize,
) -> Result<Vec<u8>, ShopBodyError<R::Error>> {
    read_body(response, limit, Overflow::KeepPrefix).await
}

/// What a read does with a body longer than its limit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Overflow {
    Refuse,
    KeepPrefix,
}

async fn read_body<R: ShopResponse>(
    mut response: R,
    limit: usize,
    overflow: Overflow,
) -> Result<Vec<u8>, ShopBodyError<R::Error>> {
    let declared = declared_length(response.headers());
    if overflow == Overflow::Refuse && declared.is_some_and(|length| length > limit as u64) {
        return Err(ShopBodyError::TooLarge { limit });
    }
    let mut body = BoundedBytes::new(limit, declared);
    loop {
        if overflow == Overflow::KeepPrefix && body.is_full() {
            return Ok(body.bytes);
        }
        let Some(chunk) = response.chunk().await.map_err(ShopBodyError::Transport)? else {
            return Ok(body.bytes);
        };
        if body.write_all(&chunk).is_err() {
            return match overflow {
                Overflow::Refuse => Err(ShopBodyError::TooLarge { limit }),
                Overflow::KeepPrefix => Ok(body.bytes),
            };
        }
    }
}

fn declared_length(headers: &HeaderMap) -> Option<u64> {
    headers
        .get(CONTENT_LENGTH)?
        .to_str()
        .ok()?
        .trim()
        .parse()
        .ok()
}

/// Body bytes up to a limit. A write past the limit keeps what fits and fails, which stops the
/// read.
struct BoundedBytes {
    bytes: Vec<u8>,
    limit: usize,
}

impl BoundedBytes {
    fn new(limit: usize, declared: Option<u64>) -> Self {
        let capacity = declared.unwrap_or_default().min(limit as u64) as usize;
        Self {
            bytes: Vec::with_capacity(capacity),
            limit,
        }
    }

    fn is_full(&self) -> bool {
        self.bytes.len() >= self.limit
    }
}

impl Write for BoundedBytes {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        let room = self.limit - self.bytes.len();
        if room == 0 {
            return Err(io::Error::other("response body exceeds its limit"));
        }
        let taken = room.min(buf.len());
        self.bytes.extend_from_slice(&buf[..taken]);
        Ok(taken)
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
