use crate::product_listing_projection_document::{
    ProductListingProjectionDocumentError, product_listing_document,
};
use application::error::box_error;
use fxrate_core::FxRateSnapshot;
use opensearch::{IndexParts, OpenSearch, http::StatusCode, params::VersionType};
use platform_opensearch::response::read_response;
use product_listing_core::product_listing_id::ProductListingId;
use product_listing_service::ports::{
    ProductListingSearchFilterMatchSource, ProductListingSearchProjection,
    ProductListingSearchProjectionWriteError, ProductListingSearchProjectionWriteOutcome,
};

const DEFAULT_INDEX: &str = "product-listings";

#[derive(Clone)]
pub struct OpenSearchProductListingSearchProjection {
    client: OpenSearch,
    index: String,
}

impl OpenSearchProductListingSearchProjection {
    pub fn new(client: OpenSearch) -> Self {
        Self {
            client,
            index: DEFAULT_INDEX.to_owned(),
        }
    }

    pub fn with_index(client: OpenSearch, index: impl Into<String>) -> Self {
        Self {
            client,
            index: index.into(),
        }
    }
}

#[async_trait::async_trait]
impl ProductListingSearchProjection for OpenSearchProductListingSearchProjection {
    async fn upsert(
        &self,
        source: &ProductListingSearchFilterMatchSource,
        sale_snapshot: Option<&FxRateSnapshot>,
    ) -> Result<ProductListingSearchProjectionWriteOutcome, ProductListingSearchProjectionWriteError>
    {
        let version = checked_source_version(source.projection_version)?;
        let document = product_listing_document(source, sale_snapshot).map_err(document_error)?;
        let document_id = document._id().to_string();
        let response = self
            .client
            .index(IndexParts::IndexId(&self.index, &document_id))
            .version(version)
            .version_type(VersionType::External)
            .body(document)
            .send()
            .await
            .map_err(
                |source| ProductListingSearchProjectionWriteError::WriteFailed {
                    source: box_error(source),
                },
            )?;
        write_response_outcome(response, true).await
    }

    async fn delete(
        &self,
        product_listing_id: ProductListingId,
        source_version: i64,
    ) -> Result<ProductListingSearchProjectionWriteOutcome, ProductListingSearchProjectionWriteError>
    {
        let version = checked_source_version(source_version)?;
        let response = self
            .client
            .index(IndexParts::IndexId(
                &self.index,
                &product_listing_id.to_string(),
            ))
            .version(version)
            .version_type(VersionType::External)
            // Physical DELETE forgets its version after index.gc_deletes (default 60s).
            // Keep this content-free document until a strictly newer source replaces it.
            .body(serde_json::json!({
                "productListingId": product_listing_id,
                "projectionDeleted": true,
            }))
            .send()
            .await
            .map_err(
                |source| ProductListingSearchProjectionWriteError::DeleteFailed {
                    source: box_error(source),
                },
            )?;
        write_response_outcome(response, false).await
    }
}

fn checked_source_version(version: i64) -> Result<i64, ProductListingSearchProjectionWriteError> {
    (version >= 1).then_some(version).ok_or_else(|| {
        ProductListingSearchProjectionWriteError::WriteFailed {
            source: application::error::static_error(
                "ProductListing projection version must be positive",
            ),
        }
    })
}

fn document_error(
    source: ProductListingProjectionDocumentError,
) -> ProductListingSearchProjectionWriteError {
    ProductListingSearchProjectionWriteError::WriteFailed {
        source: box_error(source),
    }
}

async fn write_response_outcome(
    response: opensearch::http::response::Response,
    is_write: bool,
) -> Result<ProductListingSearchProjectionWriteOutcome, ProductListingSearchProjectionWriteError> {
    let response = read_response(response).await.map_err(|source| {
        projection_write_error(is_write, application::error::box_error(source))
    })?;
    let status = response.status();
    if let Ok(outcome) = write_outcome(status, is_write) {
        return Ok(outcome);
    }
    Err(projection_write_error(
        is_write,
        application::error::box_error(std::io::Error::other(format!(
            "OpenSearch ProductListing projection returned {status}: {}",
            response.body(),
        ))),
    ))
}

fn write_outcome(
    status: StatusCode,
    is_write: bool,
) -> Result<ProductListingSearchProjectionWriteOutcome, ProductListingSearchProjectionWriteError> {
    if status == StatusCode::CONFLICT {
        return Ok(ProductListingSearchProjectionWriteOutcome::Stale);
    }
    if status.is_success() {
        return Ok(ProductListingSearchProjectionWriteOutcome::Applied);
    }
    Err(projection_write_error(
        is_write,
        application::error::static_error(
            "OpenSearch ProductListing projection returned an unsuccessful status",
        ),
    ))
}

fn projection_write_error(
    is_write: bool,
    source: application::error::BoxError,
) -> ProductListingSearchProjectionWriteError {
    if is_write {
        ProductListingSearchProjectionWriteError::WriteFailed { source }
    } else {
        ProductListingSearchProjectionWriteError::DeleteFailed { source }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_reject_non_positive_projection_version() {
        assert!(matches!(
            checked_source_version(0),
            Err(ProductListingSearchProjectionWriteError::WriteFailed { .. })
        ));
    }

    #[test]
    fn should_accept_positive_projection_version() -> Result<(), Box<dyn std::error::Error>> {
        assert_eq!(42, checked_source_version(42)?);
        Ok(())
    }

    #[test]
    fn should_map_conflict_to_stale_for_writes_and_deletes()
    -> Result<(), Box<dyn std::error::Error>> {
        assert_eq!(
            ProductListingSearchProjectionWriteOutcome::Stale,
            write_outcome(StatusCode::CONFLICT, true)?
        );
        assert_eq!(
            ProductListingSearchProjectionWriteOutcome::Stale,
            write_outcome(StatusCode::CONFLICT, false)?
        );
        Ok(())
    }

    #[test]
    fn should_map_success_to_applied_for_writes_and_deletes()
    -> Result<(), Box<dyn std::error::Error>> {
        assert_eq!(
            ProductListingSearchProjectionWriteOutcome::Applied,
            write_outcome(StatusCode::CREATED, true)?
        );
        assert_eq!(
            ProductListingSearchProjectionWriteOutcome::Applied,
            write_outcome(StatusCode::OK, false)?
        );
        Ok(())
    }

    #[test]
    fn should_map_unsuccessful_status_to_operation_specific_error() {
        assert!(matches!(
            write_outcome(StatusCode::INTERNAL_SERVER_ERROR, true),
            Err(ProductListingSearchProjectionWriteError::WriteFailed { .. })
        ));
        assert!(matches!(
            write_outcome(StatusCode::BAD_GATEWAY, false),
            Err(ProductListingSearchProjectionWriteError::DeleteFailed { .. })
        ));
    }

    mod paused_write {
        //! Test-only HTTP relay: capture a complete adapter request, then forward it unchanged in meaning
        //! to real OpenSearch after the test has applied a newer projection. No source reread on release.
        use opensearch::{
            OpenSearch,
            http::{
                Method, StatusCode, Url, headers::HeaderMap, request::JsonBody,
                transport::Transport,
            },
        };
        use std::{io, time::Duration};
        use tokio::{
            io::{AsyncReadExt, AsyncWriteExt},
            net::TcpListener,
            sync::oneshot,
            time::timeout,
        };

        type TestError = Box<dyn std::error::Error + Send + Sync>;

        pub(super) struct PausedWrite {
            pub(super) client: OpenSearch,
            received: oneshot::Receiver<()>,
            release: oneshot::Sender<()>,
            forwarded: test_api::OwnedTask<Result<StatusCode, TestError>>,
        }

        impl PausedWrite {
            pub(super) async fn new(target: OpenSearch) -> Result<Self, TestError> {
                let listener = TcpListener::bind("127.0.0.1:0").await?;
                let client = OpenSearch::new(Transport::single_node(&format!(
                    "http://{}",
                    listener.local_addr()?
                ))?);
                let (received_tx, received) = oneshot::channel();
                let (release, release_rx) = oneshot::channel();
                let forwarded = test_api::spawn_owned(async move {
                    timeout(Duration::from_secs(120), async move {
                let (mut socket, _) = listener.accept().await?;
                let mut bytes = Vec::new();
                let (header_end, content_length) = loop {
                    let count = socket.read_buf(&mut bytes).await?;
                    if count == 0 || bytes.len() > 1_000_000 {
                        return Err(io::Error::other("incomplete or oversized test request").into());
                    }
                    if let Some(end) = bytes.windows(4).position(|part| part == b"\r\n\r\n") {
                        let headers = std::str::from_utf8(&bytes[..end])?;
                        let length = headers.lines().find_map(|line| {
                            let (name, value) = line.split_once(':')?;
                            name.eq_ignore_ascii_case("content-length").then_some(value.trim())
                        });
                        break (end + 4, length.unwrap_or("0").parse::<usize>()?);
                    }
                };
                while bytes.len() < header_end + content_length {
                    if socket.read_buf(&mut bytes).await? == 0 {
                        return Err(io::Error::other("incomplete test request body").into());
                    }
                }
                let headers = std::str::from_utf8(&bytes[..header_end])?;
                let mut request_line = headers.lines().next().unwrap_or_default().split_whitespace();
                let method = match request_line.next() {
                    Some("PUT") => Method::Put,
                    Some("POST") => Method::Post,
                    Some("DELETE") => Method::Delete,
                    _ => return Err(io::Error::other("expected a projection write").into()),
                };
                let path = request_line.next().ok_or_else(|| io::Error::other("missing path"))?;
                let url = Url::parse(&format!("http://relay.test{path}"))?;
                let query = url.query_pairs().into_owned().collect::<Vec<_>>();
                let body = (content_length != 0)
                    .then(|| serde_json::from_slice::<serde_json::Value>(&bytes[header_end..header_end + content_length]))
                    .transpose()?
                    .map(JsonBody::new);
                received_tx.send(()).map_err(|_| io::Error::other("test receiver dropped"))?;
                release_rx.await?;
                let response = target.send(method, url.path(), HeaderMap::new(), Some(&query), body, None).await?;
                let status = response.status_code();
                let body = response.text().await?;
                let reply = format!(
                    "HTTP/1.1 {} Test\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    status.as_u16(), body.len(), body
                );
                socket.write_all(reply.as_bytes()).await?;
                socket.shutdown().await?;
                Ok(status)
            }).await?
                });
                Ok(Self {
                    client,
                    received,
                    release,
                    forwarded,
                })
            }

            pub(super) async fn wait_until_received(&mut self) -> Result<(), TestError> {
                if timeout(Duration::from_secs(10), &mut self.received)
                    .await?
                    .is_err()
                {
                    return match (&mut self.forwarded).await? {
                        Err(error) => Err(error),
                        Ok(_) => {
                            Err(io::Error::other("relay ended before receiving a write").into())
                        }
                    };
                }
                Ok(())
            }

            pub(super) async fn resume(self) -> Result<StatusCode, TestError> {
                self.release
                    .send(())
                    .map_err(|_| io::Error::other("test relay dropped"))?;
                timeout(Duration::from_secs(30), self.forwarded).await??
            }
        }
    }

    mod race_tests {
        use super::{OpenSearchProductListingSearchProjection, paused_write::PausedWrite};
        use crate::{
            OpenSearchProductListingSearchReader,
            OpenSearchProductListingSimilarProductListingsReader,
            product_listing_projection_document::product_listing_document,
        };
        use domain_primitives::event_id::EventId;
        use fxrate_core::{FX_RATE_SCALE, FxRateId, FxRateQuote, FxRateSource, NewFxRateSnapshot};
        use listing_source_core::{ListingSourceId, ListingSourceName, ListingSourceSlugId};
        use localization::{Language, Localized};
        use money::Currency;
        use opensearch::{
            DeleteParts, GetParts, IndexParts, http::StatusCode, indices::IndicesPutSettingsParts,
            params::VersionType,
        };
        use product_listing_core::{
            listing_lifecycle::ListingLifecycle, product_listing_id::ProductListingId,
            product_listing_search::ProductListingSearch,
            product_listing_slug_id::ProductListingSlugId, source_listing_id::SourceListingId,
            title::Title,
        };
        use product_listing_service::ports::{
            CompiledProductListingSearch, ListingSourceSummary, ProductListingPriceFilterPlan,
            ProductListingSearchFilterMatchSource, ProductListingSearchFilterMatchSourceEventKind,
            ProductListingSearchProjection, ProductListingSearchProjectionWriteOutcome,
            ProductListingSearchReadRequest, ProductListingSearchReader,
            ProductListingSimilarProductListingsReader,
            ProductListingSimilarProductListingsRequest,
        };
        use serde_json::{Value, json};
        use std::time::Duration;
        use strum::IntoEnumIterator;
        use test_api::{
            IntegrationTestService, OpenSearch as TestOpenSearch, aura_integration_test,
            get_opensearch_client, refresh_index,
        };
        use time::OffsetDateTime;

        fn products_index() -> String {
            test_api::TestContext::current()
                .expect("active projection test")
                .product_listings_index()
                .expect("scoped product listings index")
        }

        type TestResult<T = ()> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

        #[aura_integration_test(services = [TestOpenSearch()], isolated = true)]
        async fn should_fence_in_flight_writes_after_withdrawal_gc_and_restore() {
            let result = withdrawal_race().await;
            assert!(
                result.is_ok(),
                "real OpenSearch withdrawal race: {result:?}"
            );
        }

        async fn withdrawal_race() -> TestResult {
            let index = products_index();
            let client = get_opensearch_client().await.clone();
            client
                .indices()
                .put_settings(IndicesPutSettingsParts::Index(&[&index]))
                .body(json!({"index.gc_deletes": "60s"}))
                .send()
                .await?
                .error_for_status_code()?;
            let writer = OpenSearchProductListingSearchProjection::with_index(
                client.clone(),
                products_index(),
            );
            let mut source = source()?;
            assert_eq!(
                ProductListingSearchProjectionWriteOutcome::Applied,
                writer.upsert(&source, None).await?
            );
            assert_visible(&source, 1).await?;

            source.projection_version = 2;
            source.event_id = EventId::new();
            source.current_event_id = source.event_id;
            let mut paused = PausedWrite::new(client.clone()).await?;
            let delayed_writer = OpenSearchProductListingSearchProjection::with_index(
                paused.client.clone(),
                products_index(),
            );
            let stale_source = source.clone();
            let delayed =
                test_api::spawn_owned(
                    async move { delayed_writer.upsert(&stale_source, None).await },
                );
            paused.wait_until_received().await?;

            // Deletion must also fence a write when no prior projection exists.
            let mut unseen = source.clone();
            unseen.product_listing_id = ProductListingId::new();
            let unseen_id = unseen.product_listing_id;
            let mut paused_unseen = PausedWrite::new(client.clone()).await?;
            let unseen_writer = OpenSearchProductListingSearchProjection::with_index(
                paused_unseen.client.clone(),
                products_index(),
            );
            let delayed_unseen =
                test_api::spawn_owned(async move { unseen_writer.upsert(&unseen, None).await });
            paused_unseen.wait_until_received().await?;
            let (first, duplicate) = tokio::join!(
                writer.delete(source.product_listing_id, 3),
                writer.delete(source.product_listing_id, 3)
            );
            let outcomes = [first?, duplicate?];
            assert!(outcomes.contains(&ProductListingSearchProjectionWriteOutcome::Applied));
            assert!(outcomes.contains(&ProductListingSearchProjectionWriteOutcome::Stale));
            assert_eq!(
                ProductListingSearchProjectionWriteOutcome::Applied,
                writer.delete(unseen_id, 3).await?
            );

            // Control proves real DELETE version memory expired, rather than merely sleeping.
            // It carries the marker so the protocol probe cannot pollute supported searches.
            let control_id = ProductListingId::new().to_string();
            client
                .index(IndexParts::IndexId(&index, &control_id))
                .version(1)
                .version_type(VersionType::External)
                .body(json!({"projectionDeleted": true}))
                .send()
                .await?
                .error_for_status_code()?;
            client
                .delete(DeleteParts::IndexId(&index, &control_id))
                .version(3)
                .version_type(VersionType::External)
                .send()
                .await?
                .error_for_status_code()?;
            let control_write = client
                .index(IndexParts::IndexId(&index, &control_id))
                .version(2)
                .version_type(VersionType::External)
                .body(json!({"projectionDeleted": true}));
            tokio::time::sleep(Duration::from_secs(65)).await;
            assert_eq!(
                StatusCode::CREATED,
                control_write.send().await?.status_code()
            );

            assert_eq!(StatusCode::CONFLICT, paused.resume().await?);
            assert_eq!(
                ProductListingSearchProjectionWriteOutcome::Stale,
                delayed.await??
            );
            assert_eq!(StatusCode::CONFLICT, paused_unseen.resume().await?);
            assert_eq!(
                ProductListingSearchProjectionWriteOutcome::Stale,
                delayed_unseen.await??
            );
            for id in [source.product_listing_id, unseen_id] {
                let stored = stored(id).await?;
                assert_eq!(json!(id.to_string()), stored["_id"]);
                assert_eq!(json!(3), stored["_version"]);
                assert_eq!(
                    json!({"productListingId": id, "projectionDeleted": true}),
                    stored["_source"]
                );
                assert_eq!(
                    ProductListingSearchProjectionWriteOutcome::Stale,
                    writer.delete(id, 3).await?
                );
            }
            assert_visible(&source, 0).await?;

            // A delayed withdrawal cannot remove a subsequent legitimate restoration.
            let mut paused_delete = PausedWrite::new(client.clone()).await?;
            let stale_deleter = OpenSearchProductListingSearchProjection::with_index(
                paused_delete.client.clone(),
                products_index(),
            );
            let id = source.product_listing_id;
            let delayed_delete =
                test_api::spawn_owned(async move { stale_deleter.delete(id, 4).await });
            paused_delete.wait_until_received().await?;
            source.projection_version = 5;
            source.event_id = EventId::new();
            source.current_event_id = source.event_id;
            assert_eq!(
                ProductListingSearchProjectionWriteOutcome::Applied,
                writer.upsert(&source, None).await?
            );
            assert_eq!(StatusCode::CONFLICT, paused_delete.resume().await?);
            assert_eq!(
                ProductListingSearchProjectionWriteOutcome::Stale,
                delayed_delete.await??
            );
            assert_eq!(
                ProductListingSearchProjectionWriteOutcome::Stale,
                writer.upsert(&source, None).await?
            );
            let restored = stored(source.product_listing_id).await?;
            assert_eq!(
                json!(source.product_listing_id.to_string()),
                restored["_id"]
            );
            assert_eq!(json!(5), restored["_version"]);
            assert_eq!(
                serde_json::to_value(product_listing_document(&source, None)?)?,
                restored["_source"]
            );
            assert_visible(&source, 1).await?;
            Ok(())
        }

        async fn stored(id: ProductListingId) -> TestResult<Value> {
            Ok(get_opensearch_client()
                .await
                .get(GetParts::IndexId(&products_index(), &id.to_string()))
                .send()
                .await?
                .error_for_status_code()?
                .json()
                .await?)
        }

        async fn assert_visible(
            source: &ProductListingSearchFilterMatchSource,
            expected: usize,
        ) -> TestResult {
            refresh_index(&products_index()).await;
            let client = get_opensearch_client().await.clone();
            let snapshot = NewFxRateSnapshot::capture_eur(
                FxRateId::new(),
                OffsetDateTime::UNIX_EPOCH,
                FxRateSource::FxRatesApi,
                Currency::Eur,
                Currency::iter().map(|currency| FxRateQuote::new(currency, FX_RATE_SCALE)),
            )?
            .into_persisted(1_i64.try_into()?);
            let plan = ProductListingPriceFilterPlan::compile(snapshot, Currency::Eur, None)?;
            let request = ProductListingSearchReadRequest {
                compiled_search: CompiledProductListingSearch::new(
                    ProductListingSearch::new(Language::En, Currency::Eur),
                    plan.clone(),
                )?,
                sort: None,
                cursor: None,
            };
            let reader =
                OpenSearchProductListingSearchReader::with_index(client.clone(), products_index());
            let ordinary = reader.search(&request).await?;
            assert_eq!(expected, ordinary.items.len());
            assert_eq!(Some(expected as u64), ordinary.total);
            let embedding = vec![0.1; 768];
            let hybrid = reader.search_hybrid(&request, &embedding).await?;
            assert_eq!(expected, hybrid.items.len());
            let similar = OpenSearchProductListingSimilarProductListingsReader::with_index(
                client,
                products_index(),
            )
            .find_similar_product_listings(&ProductListingSimilarProductListingsRequest {
                product_listing_id: ProductListingId::new(),
                embedding,
                language: Language::En,
                price_filter_plan: plan,
            })
            .await?;
            assert_eq!(expected, similar.len());
            for item in ordinary.items.iter().chain(&hybrid.items).chain(&similar) {
                assert_eq!(source.product_listing_id, item.product_listing_id);
                assert_eq!(source.current_event_id, item.event_id);
            }
            Ok(())
        }

        pub(super) fn source() -> TestResult<ProductListingSearchFilterMatchSource> {
            let event_id = EventId::new();
            Ok(ProductListingSearchFilterMatchSource {
                event_id,
                current_event_id: event_id,
                projection_version: 1,
                shipping: Default::default(),
                geography: Default::default(),
                event_kind: ProductListingSearchFilterMatchSourceEventKind::Domain,
                origin_event_time: OffsetDateTime::UNIX_EPOCH,
                product_listing_id: ProductListingId::new(),
                product_listing_title_slug_id: ProductListingSlugId::raw("fenced-listing-a1b2c3")?,
                source: ListingSourceSummary {
                    listing_source_id: ListingSourceId::new(),
                    name: ListingSourceName::try_from("Source")?,
                    slug_id: ListingSourceSlugId::raw("source")?,
                },
                source_listing_id: SourceListingId::try_from("sku-1")?,
                product_title: Some(Localized::new(Language::En, Title::from("Fenced listing"))),
                product_description: None,
                titles: Default::default(),
                descriptions: Default::default(),
                pricing: Default::default(),
                sale_observation: None,
                availability: None,
                lifecycle: ListingLifecycle::Active,
                url: "https://example.test/listing".parse()?,
                view_url: "https://example.test/listing".parse()?,
                image: None,
                images: Default::default(),
                embedding: Some(
                    std::iter::once(1.0)
                        .chain(std::iter::repeat_n(0.0, 767))
                        .collect(),
                ),
                auction: Default::default(),
                created: OffsetDateTime::UNIX_EPOCH,
                updated: OffsetDateTime::UNIX_EPOCH,
            })
        }

        #[aura_integration_test(services=[TestOpenSearch()], isolated = true)]
        async fn geography_replacement_fences_old_points_and_preserves_country_only_queries() {
            let result:TestResult=async {
        let client=get_opensearch_client().await.clone();
        let writer=OpenSearchProductListingSearchProjection::with_index(client.clone(), products_index());
        let mut source=source()?;
        let dimension=product_listing_service::geography::GeographicDimension {
            location_id:party_core::party_location_id::PartyLocationId::new(),reference_release:geo::reference::ReferenceRelease::CURRENT,
            country:Some(geo::CountryCode::DEU),subdivision:Some(geo::SubdivisionCode::new("DE-BY")?),
            position:Some(geo::SpatialPosition::new(geo::GeoPoint::new(48.1,11.5)?,geo::SpatialPrecision::Premises,None)?),
        };
        source.geography.business=Some(dimension.clone());
        let mut paused=PausedWrite::new(client.clone()).await?;
        let old_writer=OpenSearchProductListingSearchProjection::with_index(paused.client.clone(), products_index());
        let old=source.clone();
        let stale=test_api::spawn_owned(async move {old_writer.upsert(&old,None).await});
        paused.wait_until_received().await?;
        source.projection_version+=1;
        source.geography.business.as_mut().ok_or("missing business")?.position=None;
        assert_eq!(ProductListingSearchProjectionWriteOutcome::Applied,writer.upsert(&source,None).await?);
        assert_eq!(StatusCode::CONFLICT,paused.resume().await?);
        assert_eq!(ProductListingSearchProjectionWriteOutcome::Stale,stale.await??);
        let document:Value=client.get(GetParts::IndexId(&products_index(),&source.product_listing_id.to_string())).send().await?.json().await?;
        assert_eq!("DE",document["_source"]["businessGeography"]["country"]);
        assert_eq!("DE-BY",document["_source"]["businessGeography"]["subdivision"]);
        assert!(document["_source"]["businessGeography"]["point"].is_null());
        assert!(document["_source"]["inventoryGeography"].is_null());
        // Search assertions synchronize visibility in the test, independently of writer completion.
        refresh_index(&products_index()).await;
        let result:Value=client.search(opensearch::SearchParts::Index(&[&products_index()])).body(json!({"query":{"term":{"businessGeography.country":"DE"}},"aggs":{"countries":{"terms":{"field":"businessGeography.country"}}}})).send().await?.json().await?;
        assert_eq!(1,result["hits"]["total"]["value"]);assert_eq!("DE",result["aggregations"]["countries"]["buckets"][0]["key"]);
        let document=product_listing_document(&product_listing_service::ports::ProductListingSearchFilterMatchSource {geography:product_listing_service::geography::EffectiveListingGeography{business:Some(dimension),inventory:None},..source.clone()},None)?;
        let value=serde_json::to_value(document)?;assert_eq!(48.1,value["businessGeography"]["point"]["lat"]);assert_eq!(11.5,value["businessGeography"]["point"]["lon"]);
        Ok(())
    }.await;
            assert!(
                result.is_ok(),
                "geo external fence and country discovery: {result:?}"
            );
        }

        #[aura_integration_test(services=[TestOpenSearch()], isolated = true)]
        async fn shipping_nested_country_price_cannot_cross_match_and_stale_complete_writes_are_fenced()
         {
            let result: TestResult = async {
                use product_listing_service::shipping_projection::{
                    CountryShippingOffer, EffectiveListingShipping,
                };
                use shipping_core::terms::{ShippingEligibility, ShippingServiceCode};

                let client = get_opensearch_client().await.clone();
                let writer = OpenSearchProductListingSearchProjection::with_index(
                    client.clone(),
                    products_index(),
                );
                let mut listing = source()?;
                listing.shipping = EffectiveListingShipping {
                    collection: Some(ShippingEligibility::Offered),
                    prohibited: false,
                    conditional: false,
                    country_offers: vec![
                        CountryShippingOffer {
                            country: geo::CountryCode::DEU,
                            service: ShippingServiceCode::new("de-service".into())?,
                            per_item_fixed: Some(money::Price::new(8000_u64.into(), Currency::Eur)),
                        },
                        CountryShippingOffer {
                            country: geo::CountryCode::FRA,
                            service: ShippingServiceCode::new("fr-service".into())?,
                            per_item_fixed: Some(money::Price::new(5000_u64.into(), Currency::Eur)),
                        },
                    ],
                };
                writer.upsert(&listing, None).await?;
                refresh_index(&products_index()).await;
                for (country, amount, count) in [
                    (geo::CountryCode::DEU, 6000_u64, 0),
                    (geo::CountryCode::FRA, 6000, 1),
                    (geo::CountryCode::DEU, 8000, 1),
                ] {
                    let predicate = crate::shipping_query::country_offer_query(
                        country,
                        None,
                        Some(money::Price::new(amount.into(), Currency::Eur)),
                    );
                    let response: Value = client
                        .search(opensearch::SearchParts::Index(&[&products_index()]))
                        .body(json!({"query":{"bool":{
                            "filter":[predicate],
                            "must_not":[{"term":{"projectionDeleted":true}}]
                        }}}))
                        .send()
                        .await?
                        .error_for_status_code()?
                        .json()
                        .await?;
                    assert_eq!(count, response["hits"]["total"]["value"]);
                }
                // The actual strict mapping rejects unknown private fields inside an offer.
                let mut poison = serde_json::to_value(product_listing_document(&listing, None)?)?;
                poison["shipping"]["countryOffers"][0]["address"] = json!("forbidden");
                let rejected = client
                    .index(IndexParts::IndexId(
                        &products_index(),
                        &ProductListingId::new().to_string(),
                    ))
                    .body(poison)
                    .send()
                    .await?;
                assert_eq!(StatusCode::BAD_REQUEST, rejected.status_code());

                let mut paused = PausedWrite::new(client.clone()).await?;
                let old_writer = OpenSearchProductListingSearchProjection::with_index(
                    paused.client.clone(),
                    products_index(),
                );
                let old = listing.clone();
                let stale =
                    test_api::spawn_owned(async move { old_writer.upsert(&old, None).await });
                paused.wait_until_received().await?;
                listing.projection_version = 2;
                listing.shipping = EffectiveListingShipping {
                    prohibited: true,
                    ..Default::default()
                };
                listing.pricing.price =
                    Some(money::Price::new(12345_u64.into(), Currency::Eur).into());
                listing.geography.business =
                    Some(product_listing_service::geography::GeographicDimension {
                        location_id: party_core::party_location_id::PartyLocationId::new(),
                        reference_release: geo::reference::ReferenceRelease::CURRENT,
                        country: Some(geo::CountryCode::FRA),
                        subdivision: None,
                        position: None,
                    });
                assert_eq!(
                    ProductListingSearchProjectionWriteOutcome::Applied,
                    writer.upsert(&listing, None).await?
                );
                assert_eq!(StatusCode::CONFLICT, paused.resume().await?);
                assert_eq!(
                    ProductListingSearchProjectionWriteOutcome::Stale,
                    stale.await??
                );
                let current: Value = client
                    .get(GetParts::IndexId(
                        &products_index(),
                        &listing.product_listing_id.to_string(),
                    ))
                    .send()
                    .await?
                    .json()
                    .await?;
                assert_eq!(true, current["_source"]["shipping"]["prohibited"]);
                assert_eq!(json!([]), current["_source"]["shipping"]["countryOffers"]);
                assert_eq!(12345, current["_source"]["sourcePrice"]["amount"]);
                assert_eq!("FR", current["_source"]["businessGeography"]["country"]);

                writer.delete(listing.product_listing_id, 3).await?;
                assert_eq!(
                    ProductListingSearchProjectionWriteOutcome::Stale,
                    writer.upsert(&listing, None).await?
                );
                let deleted: Value = client
                    .get(GetParts::IndexId(
                        &products_index(),
                        &listing.product_listing_id.to_string(),
                    ))
                    .send()
                    .await?
                    .json()
                    .await?;
                assert_eq!(
                    json!({"productListingId":listing.product_listing_id,"projectionDeleted":true}),
                    deleted["_source"]
                );
                refresh_index(&products_index()).await;
                let visible: Value = client
            .search(opensearch::SearchParts::Index(&[&products_index()]))
            .body(json!({"query":{"bool":{"must_not":[{"term":{"projectionDeleted":true}}]}}}))
            .send()
            .await?
            .json()
            .await?;
                assert_eq!(0, visible["hits"]["total"]["value"]);
                Ok(())
            }
            .await;
            assert!(
                result.is_ok(),
                "shipping nested discovery and complete projection fence: {result:?}"
            );
        }
    }

    mod request_tests {
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
    }
}
