use super::{OpenSearchProductListingSearchProjection, race_tests::source};
use opensearch::{OpenSearch, http::transport::Transport};
use product_listing_service::ports::{
    ProductListingSearchProjection, ProductListingSearchProjectionWriteOutcome,
};
use rstest::rstest;
use std::{io, time::Duration};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
    time::timeout,
};
use url::Url;

type TestResult = Result<(), Box<dyn std::error::Error + Send + Sync>>;

#[rstest]
#[case(false, 201, ProductListingSearchProjectionWriteOutcome::Applied)]
#[case(true, 201, ProductListingSearchProjectionWriteOutcome::Applied)]
#[case(false, 409, ProductListingSearchProjectionWriteOutcome::Stale)]
#[case(true, 409, ProductListingSearchProjectionWriteOutcome::Stale)]
#[tokio::test]
async fn should_complete_versioned_write_without_refresh(
    #[case] tombstone: bool,
    #[case] status: u16,
    #[case] expected: ProductListingSearchProjectionWriteOutcome,
) -> TestResult {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let client = OpenSearch::new(Transport::single_node(&format!(
        "http://{}",
        listener.local_addr()?
    ))?);
    let source = source()?;
    let id = source.product_listing_id;
    let version = source.projection_version;
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await?;
        // Any follow-up refresh/probe must fail instead of acquiring visibility.
        drop(listener);
        let mut bytes = Vec::new();
        let (header_end, content_length) = loop {
            if socket.read_buf(&mut bytes).await? == 0 || bytes.len() > 1_000_000 {
                return Err(io::Error::other(
                    "incomplete or oversized projection request",
                ));
            }
            if let Some(end) = bytes.windows(4).position(|part| part == b"\r\n\r\n") {
                let headers = String::from_utf8_lossy(&bytes[..end]);
                let length = headers.lines().find_map(|line| {
                    let (name, value) = line.split_once(':')?;
                    name.eq_ignore_ascii_case("content-length")
                        .then_some(value.trim())
                });
                let length = length
                    .unwrap_or("0")
                    .parse::<usize>()
                    .map_err(io::Error::other)?;
                break (end + 4, length);
            }
        };
        while bytes.len() < header_end + content_length {
            if socket.read_buf(&mut bytes).await? == 0 {
                return Err(io::Error::other("incomplete projection request body"));
            }
        }
        let headers = String::from_utf8_lossy(&bytes[..header_end]);
        let mut request = headers
            .lines()
            .next()
            .unwrap_or_default()
            .split_whitespace();
        assert_eq!(Some("POST"), request.next());
        let url = Url::parse(&format!(
            "http://test{}",
            request.next().unwrap_or_default()
        ))
        .map_err(io::Error::other)?;
        assert_eq!(format!("/product-listings/_doc/{id}"), url.path());
        let query = url
            .query_pairs()
            .collect::<std::collections::HashMap<_, _>>();
        assert_eq!(
            Some("external"),
            query.get("version_type").map(|v| v.as_ref())
        );
        assert_eq!(
            Some(version.to_string().as_str()),
            query.get("version").map(|v| v.as_ref())
        );
        assert!(!query.contains_key("refresh"));
        let document: serde_json::Value =
            serde_json::from_slice(&bytes[header_end..header_end + content_length])
                .map_err(io::Error::other)?;
        if tombstone {
            assert_eq!(
                serde_json::json!({"productListingId": id, "projectionDeleted": true}),
                document
            );
        } else {
            assert_eq!(serde_json::json!(id), document["productListingId"]);
            assert_ne!(serde_json::json!(true), document["projectionDeleted"]);
        }
        socket.write_all(format!(
            "HTTP/1.1 {status} Test\r\nContent-Type: application/json\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{{}}",
        ).as_bytes()).await?;
        socket.shutdown().await
    });
    let writer = OpenSearchProductListingSearchProjection::new(client);
    let outcome = timeout(Duration::from_secs(5), async {
        if tombstone {
            writer.delete(id, version).await
        } else {
            writer.upsert(&source, None).await
        }
    })
    .await;
    timeout(Duration::from_secs(5), server).await???;
    assert_eq!(expected, outcome??);
    Ok(())
}
