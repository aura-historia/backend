//! Reads the body of a shop's answer within a size limit, through one implementation for every
//! crawler client that reads shop bodies: the scraper's, and the spider's own requests on the
//! reqwest major version Spider re-exports.
//!
//! Those clients turn reqwest's transparent decompression off, which hides a compressed body's
//! `Content-Length`, so the reader sees the body as the shop framed it:
//!
//! - A body with neither `Content-Length` nor `Transfer-Encoding` ends when the shop closes the
//!   connection. Some shops close TLS there without `close_notify`, which rustls reports as an
//!   unexpected end of file; browsers and curl accept such a body, and so does the reader. Any
//!   other body that ends early fails, as does a compressed stream cut off before its end (gzip
//!   trailer, zlib checksum, Brotli end marker).
//! - The reader decodes `gzip`, `deflate` and `br` itself as the chunks arrive, so the limit
//!   counts decoded bytes and a decompression bomb stops at it
//!   (`docs/crawler/networking.md#request-headers`). As browsers and reqwest's own decoding do,
//!   an encoded body without a byte reads as empty, and the bytes after a compressed stream's end
//!   are left out.

use crate::network::policy::ReqwestErrorLike;
use hyper::body::Bytes;
use reqwest::Version;
use reqwest::header::{CONTENT_ENCODING, CONTENT_LENGTH, HeaderMap, TRANSFER_ENCODING};
use std::future::Future;
use std::io::{self, Write};

/// A response whose body the crawler reads from a shop.
pub trait ShopResponse {
    type Error: ReqwestErrorLike + Send + Sync;

    fn headers(&self) -> &HeaderMap;

    fn version(&self) -> Version;

    /// The next chunk of the body as it came off the connection; `None` at its end.
    fn chunk(&mut self) -> impl Future<Output = Result<Option<Bytes>, Self::Error>> + Send;
}

impl ShopResponse for reqwest::Response {
    type Error = reqwest::Error;

    fn headers(&self) -> &HeaderMap {
        reqwest::Response::headers(self)
    }

    fn version(&self) -> Version {
        reqwest::Response::version(self)
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
    /// The body is corrupt, or its compressed stream ends early.
    #[error("response body does not decode as {coding}")]
    Decode {
        coding: &'static str,
        #[source]
        source: io::Error,
    },
}

/// The whole decoded body, failing with [`ShopBodyError::TooLarge`] when it decodes to more than
/// `limit` bytes or an unencoded body declares more.
pub async fn read_complete_body<R: ShopResponse>(
    response: R,
    limit: usize,
) -> Result<Vec<u8>, ShopBodyError<R::Error>> {
    read_body(response, limit, Overflow::Refuse).await
}

/// The first `limit` decoded bytes of the body; the rest is not read.
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
    let framing = Framing::of(response.version(), response.headers());
    let coding = ContentCoding::of(response.headers());
    let declared = coding.declared_decoded_length(response.headers());
    if overflow == Overflow::Refuse && declared.is_some_and(|length| length > limit as u64) {
        return Err(ShopBodyError::TooLarge { limit });
    }
    let mut decoder = BodyDecoder::new(coding, BoundedBytes::new(limit, declared));
    loop {
        if overflow == Overflow::KeepPrefix && decoder.decoded().is_full() {
            return Ok(decoder.into_bytes());
        }
        let chunk = match response.chunk().await {
            Ok(Some(chunk)) => chunk,
            Ok(None) => break,
            Err(error) if framing == Framing::UntilClose && ended_without_close_notify(&error) => {
                break;
            }
            Err(error) => return Err(ShopBodyError::Transport(error)),
        };
        if let Err(error) = decoder.write(&chunk) {
            return decoder.stopped(error, overflow);
        }
    }
    match decoder.finish() {
        Ok(()) => Ok(decoder.into_bytes()),
        Err(error) => decoder.stopped(error, overflow),
    }
}

/// How the shop marks the end of the body, as hyper reads it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Framing {
    /// `Content-Length`, chunked transfer coding, or an HTTP/2 stream end.
    Delimited,
    /// An HTTP/1.x body that ends when the shop closes the connection: one with neither header,
    /// or with a `Transfer-Encoding` whose last coding is not `chunked`.
    UntilClose,
}

impl Framing {
    fn of(version: Version, headers: &HeaderMap) -> Self {
        if !matches!(version, Version::HTTP_10 | Version::HTTP_11) {
            return Self::Delimited;
        }
        match last_transfer_coding(headers) {
            Some(coding) if coding.eq_ignore_ascii_case("chunked") => Self::Delimited,
            Some(_) => Self::UntilClose,
            None if headers.contains_key(CONTENT_LENGTH) => Self::Delimited,
            None => Self::UntilClose,
        }
    }
}

/// The last coding the response's `Transfer-Encoding` names, which decides its framing; `None`
/// without the header. A `Content-Length` beside it does not count, as for hyper.
fn last_transfer_coding(headers: &HeaderMap) -> Option<String> {
    let values: Vec<&str> = headers
        .get_all(TRANSFER_ENCODING)
        .iter()
        .map(|value| value.to_str().unwrap_or_default())
        .collect();
    if values.is_empty() {
        return None;
    }
    Some(
        values
            .iter()
            .flat_map(|value| value.split(','))
            .map(str::trim)
            .rfind(|coding| !coding.is_empty())
            .unwrap_or_default()
            .to_owned(),
    )
}

/// Whether the read failed because the peer closed the TLS connection without `close_notify`,
/// which rustls reports as an unexpected end of file. Plain HTTP ends such a body cleanly.
fn ended_without_close_notify(error: &(dyn std::error::Error + 'static)) -> bool {
    std::iter::successors(Some(error), |error| error.source()).any(|error| {
        error
            .downcast_ref::<io::Error>()
            .is_some_and(|error| error.kind() == io::ErrorKind::UnexpectedEof)
    })
}

/// The `Content-Encoding` the reader decodes. Any other value, stacked codings included, passes
/// through undecoded, as under reqwest's transparent decompression.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ContentCoding {
    Identity,
    Gzip,
    Deflate,
    Brotli,
}

impl ContentCoding {
    fn of(headers: &HeaderMap) -> Self {
        let Some(value) = headers
            .get(CONTENT_ENCODING)
            .and_then(|value| value.to_str().ok())
        else {
            return Self::Identity;
        };
        match value.trim().to_ascii_lowercase().as_str() {
            "gzip" | "x-gzip" => Self::Gzip,
            "deflate" => Self::Deflate,
            "br" => Self::Brotli,
            _ => Self::Identity,
        }
    }

    const fn name(self) -> &'static str {
        match self {
            Self::Identity => "identity",
            Self::Gzip => "gzip",
            Self::Deflate => "deflate",
            Self::Brotli => "br",
        }
    }

    /// The decoded length the shop declares, which only an unencoded body's `Content-Length` is,
    /// and only without a `Transfer-Encoding`, which overrides it.
    fn declared_decoded_length(self, headers: &HeaderMap) -> Option<u64> {
        if self != Self::Identity || headers.contains_key(TRANSFER_ENCODING) {
            return None;
        }
        headers
            .get(CONTENT_LENGTH)?
            .to_str()
            .ok()?
            .trim()
            .parse()
            .ok()
    }
}

/// Decodes the body chunk by chunk into [`BoundedBytes`], whose refusal past the limit stops the
/// decoder before it produces more. As browsers and reqwest's own decoding do, a body without a
/// byte is empty whatever its coding, and the bytes after the end of a compressed stream, such as
/// a trailing newline or a second gzip member, are not read.
struct BodyDecoder {
    stream: DecodingStream,
    /// Whether any encoded byte arrived.
    received: bool,
    /// Whether the compressed stream reached its end.
    ended: bool,
}

enum DecodingStream {
    Identity(BoundedBytes),
    Gzip(flate2::write::GzDecoder<BoundedBytes>),
    Deflate(ZlibStream),
    Brotli(Box<brotli::DecompressorWriter<BoundedBytes>>),
}

/// The Brotli decoder's output buffer.
const BROTLI_BUFFER_BYTES: usize = 8 * 1024;

impl BodyDecoder {
    fn new(coding: ContentCoding, decoded: BoundedBytes) -> Self {
        let stream = match coding {
            ContentCoding::Identity => DecodingStream::Identity(decoded),
            ContentCoding::Gzip => DecodingStream::Gzip(flate2::write::GzDecoder::new(decoded)),
            ContentCoding::Deflate => DecodingStream::Deflate(ZlibStream::new(decoded)),
            ContentCoding::Brotli => DecodingStream::Brotli(Box::new(
                brotli::DecompressorWriter::new(decoded, BROTLI_BUFFER_BYTES),
            )),
        };
        Self {
            stream,
            received: false,
            ended: false,
        }
    }

    fn coding(&self) -> ContentCoding {
        match &self.stream {
            DecodingStream::Identity(_) => ContentCoding::Identity,
            DecodingStream::Gzip(_) => ContentCoding::Gzip,
            DecodingStream::Deflate(_) => ContentCoding::Deflate,
            DecodingStream::Brotli(_) => ContentCoding::Brotli,
        }
    }

    /// Decodes `encoded`; once the compressed stream ended, what follows is not read.
    fn write(&mut self, encoded: &[u8]) -> io::Result<()> {
        if encoded.is_empty() {
            return Ok(());
        }
        self.received = true;
        if self.ended {
            return Ok(());
        }
        self.ended = match &mut self.stream {
            DecodingStream::Identity(decoded) => decoded.write_all(encoded).map(|()| false)?,
            DecodingStream::Gzip(decoder) => write_until_stream_end(decoder, encoded)?,
            DecodingStream::Deflate(decoder) => decoder.write(encoded)?,
            DecodingStream::Brotli(decoder) => write_until_stream_end(decoder.as_mut(), encoded)?,
        };
        Ok(())
    }

    /// Writes out what the decoder holds and fails when the compressed stream has not ended. A
    /// body without a byte is empty.
    fn finish(&mut self) -> io::Result<()> {
        if !self.received || self.ended {
            return Ok(());
        }
        match &mut self.stream {
            DecodingStream::Identity(_) => Ok(()),
            DecodingStream::Gzip(decoder) => decoder.try_finish(),
            DecodingStream::Deflate(decoder) => decoder.finish(),
            DecodingStream::Brotli(decoder) => decoder.close(),
        }
    }

    fn decoded(&self) -> &BoundedBytes {
        match &self.stream {
            DecodingStream::Identity(decoded) => decoded,
            DecodingStream::Gzip(decoder) => decoder.get_ref(),
            DecodingStream::Deflate(decoder) => &decoder.decoded,
            DecodingStream::Brotli(decoder) => decoder.get_ref(),
        }
    }

    fn into_bytes(mut self) -> Vec<u8> {
        let decoded = match &mut self.stream {
            DecodingStream::Identity(decoded) => decoded,
            DecodingStream::Gzip(decoder) => decoder.get_mut(),
            DecodingStream::Deflate(decoder) => &mut decoder.decoded,
            DecodingStream::Brotli(decoder) => decoder.get_mut(),
        };
        std::mem::take(&mut decoded.bytes)
    }

    /// What a failed write or finish means: the decoded body passed the limit, or the body does
    /// not decode.
    fn stopped<E: std::error::Error + 'static>(
        self,
        error: io::Error,
        overflow: Overflow,
    ) -> Result<Vec<u8>, ShopBodyError<E>> {
        let decoded = self.decoded();
        if !decoded.exceeded {
            return Err(ShopBodyError::Decode {
                coding: self.coding().name(),
                source: error,
            });
        }
        match overflow {
            Overflow::Refuse => Err(ShopBodyError::TooLarge {
                limit: decoded.limit,
            }),
            Overflow::KeepPrefix => Ok(self.into_bytes()),
        }
    }
}

/// Writes `encoded` to a decoding `writer` until it is all taken or the writer takes no more
/// because its compressed stream ended, which it answers.
fn write_until_stream_end(writer: &mut impl Write, mut encoded: &[u8]) -> io::Result<bool> {
    while !encoded.is_empty() {
        match writer.write(encoded) {
            Ok(0) => return Ok(true),
            Ok(taken) => encoded = &encoded[taken..],
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error) => return Err(error),
        }
    }
    Ok(false)
}

/// A zlib stream (`Content-Encoding: deflate`), which flate2's writer would let end anywhere: the
/// stream counts as complete only once the decoder reached its end and checked its checksum.
struct ZlibStream {
    inflate: flate2::Decompress,
    decoded: BoundedBytes,
    ended: bool,
}

impl ZlibStream {
    fn new(decoded: BoundedBytes) -> Self {
        Self {
            inflate: flate2::Decompress::new(true),
            decoded,
            ended: false,
        }
    }

    /// Decodes `encoded` and answers whether the stream ended; the bytes after its end are not
    /// read.
    fn write(&mut self, mut encoded: &[u8]) -> io::Result<bool> {
        let mut buffer = [0_u8; 8 * 1024];
        loop {
            if self.ended {
                return Ok(true);
            }
            let (read_before, written_before) = (self.inflate.total_in(), self.inflate.total_out());
            let status = self
                .inflate
                .decompress(encoded, &mut buffer, flate2::FlushDecompress::None)
                .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
            let read = (self.inflate.total_in() - read_before) as usize;
            let written = (self.inflate.total_out() - written_before) as usize;
            self.decoded.write_all(&buffer[..written])?;
            encoded = &encoded[read..];
            self.ended = status == flate2::Status::StreamEnd;
            if self.ended {
                continue;
            }
            if encoded.is_empty() && written < buffer.len() {
                return Ok(false);
            }
            if read == 0 && written == 0 {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "deflate stream makes no progress",
                ));
            }
        }
    }

    fn finish(&mut self) -> io::Result<()> {
        if self.ended {
            Ok(())
        } else {
            Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "deflate stream ends before its end",
            ))
        }
    }
}

/// Decoded bytes up to a limit. A write past the limit keeps what fits, fails and marks the body
/// as exceeding it.
struct BoundedBytes {
    bytes: Vec<u8>,
    limit: usize,
    exceeded: bool,
}

impl BoundedBytes {
    fn new(limit: usize, declared: Option<u64>) -> Self {
        let capacity = declared.unwrap_or_default().min(limit as u64) as usize;
        Self {
            bytes: Vec::with_capacity(capacity),
            limit,
            exceeded: false,
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
            self.exceeded = true;
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::network::policy::{RequestTimeouts, shop_client_builder};
    use crate::test_support::local_tls;
    use rstest::rstest;
    use std::net::SocketAddr;
    use std::time::Duration;

    const ROBOTS_TXT: &[u8] = b"User-agent: *\nDisallow: /private/\nAllow: /\n";

    fn gzip(body: &[u8]) -> Vec<u8> {
        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        encoder.write_all(body).unwrap();
        encoder.finish().unwrap()
    }

    fn zlib(body: &[u8]) -> Vec<u8> {
        let mut encoder = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::fast());
        encoder.write_all(body).unwrap();
        encoder.finish().unwrap()
    }

    fn brotli(body: &[u8]) -> Vec<u8> {
        let mut compressed = Vec::new();
        {
            let mut encoder = brotli::CompressorWriter::new(&mut compressed, 4096, 5, 22);
            encoder.write_all(body).unwrap();
        }
        compressed
    }

    fn encode(coding: &str, body: &[u8]) -> Vec<u8> {
        match coding {
            "identity" => body.to_vec(),
            "gzip" => gzip(body),
            "deflate" => zlib(body),
            "br" => brotli(body),
            _ => unreachable!("{coding}"),
        }
    }

    /// An HTTP/1.1 answer with `headers` (each `Name: value\r\n`) and `body` as it goes out.
    fn answer(headers: &str, body: &[u8]) -> Vec<u8> {
        let mut answer =
            format!("HTTP/1.1 200 OK\r\n{headers}Connection: close\r\n\r\n").into_bytes();
        answer.extend_from_slice(body);
        answer
    }

    /// A body with neither `Content-Length` nor `Transfer-Encoding`, encoded with `coding`.
    fn close_delimited(coding: &str, encoded: &[u8]) -> Vec<u8> {
        answer(&format!("Content-Encoding: {coding}\r\n"), encoded)
    }

    async fn get(addr: SocketAddr) -> reqwest::Response {
        shop_client_builder(RequestTimeouts::uniform(Duration::from_secs(5)))
            .unwrap()
            .use_preconfigured_tls(local_tls::client_config())
            .resolve("localhost", addr)
            .build()
            .unwrap()
            .get(format!("https://localhost:{}/robots.txt", addr.port()))
            .send()
            .await
            .unwrap()
    }

    async fn read_complete(
        answer: Vec<u8>,
        limit: usize,
    ) -> Result<Vec<u8>, ShopBodyError<reqwest::Error>> {
        let addr = local_tls::serve_without_close_notify(answer, 1);
        read_complete_body(get(addr).await, limit).await
    }

    /// The test server reproduces the shops of #3008: reqwest alone fails their body.
    #[tokio::test]
    async fn should_fail_a_plain_reqwest_read_when_tls_ends_a_close_delimited_body_without_close_notify()
     {
        let addr = local_tls::serve_without_close_notify(answer("", ROBOTS_TXT), 1);

        let error = get(addr).await.bytes().await.unwrap_err();

        assert!(ended_without_close_notify(&error), "{error:?}");
    }

    #[rstest]
    #[case::identity("identity")]
    #[case::gzip("gzip")]
    #[case::deflate("deflate")]
    #[case::brotli("br")]
    #[tokio::test]
    async fn should_read_a_close_delimited_body_when_tls_ends_without_close_notify(
        #[case] coding: &str,
    ) {
        let body = read_complete(close_delimited(coding, &encode(coding, ROBOTS_TXT)), 1024)
            .await
            .unwrap();

        assert_eq!(body, ROBOTS_TXT);
    }

    #[tokio::test]
    async fn should_fail_when_a_content_length_body_ends_early_without_close_notify() {
        let headers = format!("Content-Length: {}\r\n", ROBOTS_TXT.len() + 10);

        let error = read_complete(answer(&headers, ROBOTS_TXT), 1024)
            .await
            .unwrap_err();

        assert!(matches!(error, ShopBodyError::Transport(_)), "{error:?}");
    }

    #[tokio::test]
    async fn should_fail_when_a_chunked_body_ends_early_without_close_notify() {
        // One complete chunk and no last chunk.
        let mut chunked = format!("{:x}\r\n", ROBOTS_TXT.len()).into_bytes();
        chunked.extend_from_slice(ROBOTS_TXT);
        chunked.extend_from_slice(b"\r\n");

        let error = read_complete(answer("Transfer-Encoding: chunked\r\n", &chunked), 1024)
            .await
            .unwrap_err();

        assert!(matches!(error, ShopBodyError::Transport(_)), "{error:?}");
    }

    #[tokio::test]
    async fn should_fail_when_a_compressed_content_length_body_ends_early_without_close_notify() {
        let encoded = gzip(ROBOTS_TXT);
        let headers = format!(
            "Content-Encoding: gzip\r\nContent-Length: {}\r\n",
            encoded.len()
        );

        let error = read_complete(answer(&headers, &encoded[..encoded.len() / 2]), 1024)
            .await
            .unwrap_err();

        assert!(matches!(error, ShopBodyError::Transport(_)), "{error:?}");
    }

    /// The compressed stream itself proves a close-delimited body cut off: gzip's trailer, the
    /// zlib checksum and Brotli's end marker are missing.
    #[rstest]
    #[case::gzip("gzip")]
    #[case::deflate("deflate")]
    #[case::brotli("br")]
    #[tokio::test]
    async fn should_fail_when_a_close_delimited_compressed_body_ends_before_its_stream(
        #[case] coding: &str,
    ) {
        let encoded = encode(coding, ROBOTS_TXT);

        let error = read_complete(close_delimited(coding, &encoded[..encoded.len() - 4]), 1024)
            .await
            .unwrap_err();

        assert!(matches!(error, ShopBodyError::Decode { .. }), "{error:?}");
    }

    /// A body that decodes to far more than the limit stops at the limit, whatever its declared,
    /// compressed length.
    #[rstest]
    #[case::gzip("gzip")]
    #[case::deflate("deflate")]
    #[case::brotli("br")]
    #[tokio::test]
    async fn should_stop_decoding_at_the_limit_when_a_compressed_body_decodes_past_it(
        #[case] coding: &str,
    ) {
        const LIMIT: usize = 64 * 1024;
        let bomb = encode(coding, &vec![0; 64 * LIMIT]);
        let headers = format!(
            "Content-Encoding: {coding}\r\nContent-Length: {}\r\n",
            bomb.len()
        );
        assert!(
            bomb.len() < LIMIT,
            "the compressed body is within the limit"
        );
        let addr = local_tls::serve_without_close_notify(answer(&headers, &bomb), 2);

        let complete = read_complete_body(get(addr).await, LIMIT).await;
        let prefix = read_body_prefix(get(addr).await, LIMIT).await.unwrap();

        assert!(
            matches!(complete, Err(ShopBodyError::TooLarge { limit: LIMIT })),
            "{complete:?}"
        );
        assert_eq!(prefix, vec![0; LIMIT]);
    }

    /// As reqwest's own decoding did, an encoded body without a byte reads as empty: a robots.txt
    /// that answers `Content-Encoding: gzip` with nothing must not fail the whole shop.
    #[rstest]
    #[case::gzip("gzip")]
    #[case::deflate("deflate")]
    #[case::brotli("br")]
    #[tokio::test]
    async fn should_read_an_empty_body_when_it_names_a_content_encoding(#[case] coding: &str) {
        let headers = format!("Content-Encoding: {coding}\r\nContent-Length: 0\r\n");

        let body = read_complete(answer(&headers, b""), 1024).await.unwrap();

        assert!(body.is_empty());
    }

    /// The bytes after a compressed stream's end, such as the newline a PHP shop adds or a second
    /// gzip member, are not read, as browsers and reqwest's own decoding do.
    #[rstest]
    #[case::gzip_newline("gzip", b"\n".to_vec())]
    #[case::deflate_newline("deflate", b"\n".to_vec())]
    #[case::brotli_newline("br", b"\n".to_vec())]
    #[case::second_gzip_member("gzip", gzip(b"Disallow: /second-member/\n"))]
    #[tokio::test]
    async fn should_read_the_stream_and_leave_out_bytes_after_its_end(
        #[case] coding: &str,
        #[case] trailing: Vec<u8>,
    ) {
        let mut encoded = encode(coding, ROBOTS_TXT);
        encoded.extend_from_slice(&trailing);
        let headers = format!(
            "Content-Encoding: {coding}\r\nContent-Length: {}\r\n",
            encoded.len()
        );

        let body = read_complete(answer(&headers, &encoded), 1024)
            .await
            .unwrap();

        assert_eq!(body, ROBOTS_TXT);
    }

    /// hyper reads a body whose `Transfer-Encoding` does not end in `chunked` until the shop
    /// closes the connection, so the missing `close_notify` ends it too.
    #[tokio::test]
    async fn should_read_a_body_framed_by_a_non_chunked_transfer_encoding_until_close() {
        let body = read_complete(answer("Transfer-Encoding: identity\r\n", ROBOTS_TXT), 1024)
            .await
            .unwrap();

        assert_eq!(body, ROBOTS_TXT);
    }

    #[tokio::test]
    async fn should_read_the_prefix_of_a_close_delimited_compressed_body() {
        let addr =
            local_tls::serve_without_close_notify(close_delimited("gzip", &gzip(ROBOTS_TXT)), 1);

        let prefix = read_body_prefix(get(addr).await, 12).await.unwrap();

        assert_eq!(prefix, &ROBOTS_TXT[..12]);
    }

    /// A `Transfer-Encoding` overrides a `Content-Length` beside it, for the framing and for the
    /// length the body declares.
    #[rstest]
    #[case::content_length(&[("content-length", "12")], Framing::Delimited, Some(12))]
    #[case::chunked_with_length(
        &[("transfer-encoding", "gzip, Chunked"), ("content-length", "99999999")],
        Framing::Delimited,
        None
    )]
    #[case::not_chunked(&[("transfer-encoding", "identity")], Framing::UntilClose, None)]
    #[case::neither(&[], Framing::UntilClose, None)]
    fn should_frame_a_response_and_read_its_declared_length_as_hyper_does(
        #[case] pairs: &[(&'static str, &'static str)],
        #[case] framing: Framing,
        #[case] declared: Option<u64>,
    ) {
        let headers: HeaderMap = pairs
            .iter()
            .map(|(name, value)| {
                (
                    reqwest::header::HeaderName::from_static(name),
                    reqwest::header::HeaderValue::from_static(value),
                )
            })
            .collect();

        assert_eq!(framing, Framing::of(Version::HTTP_11, &headers));
        assert_eq!(framing, Framing::of(Version::HTTP_10, &headers));
        assert_eq!(Framing::Delimited, Framing::of(Version::HTTP_2, &headers));
        assert_eq!(
            declared,
            ContentCoding::Identity.declared_decoded_length(&headers)
        );
    }
}
