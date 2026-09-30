//! Shopify upstream acknowledgement must reflect the real SDK publisher's custody report.
//! Only the HTTP connector is replaced; request signing, serialization and receipt parsing run.

use aws_lambda_events::sqs::{SqsBatchResponse, SqsEvent, SqsMessage};
use aws_sdk_sqs::config::{Credentials, Region};
use aws_smithy_runtime_api::{
    client::{
        http::{
            HttpClient, HttpConnector, HttpConnectorFuture, HttpConnectorSettings,
            SharedHttpConnector,
        },
        orchestrator::{HttpRequest, HttpResponse},
        result::ConnectorError,
        runtime_components::RuntimeComponents,
    },
    shared::IntoShared,
};
use aws_smithy_types::body::SdkBody;
use lambda_runtime::{Context, LambdaEvent};
use listing_source_core::{Domain, ListingSourceId};
use listing_source_service::ports::{ListingSourceReadError, ShopifySource, ShopifySourceReader};
use product_listing_ingestion_sqs::{
    ScopedSqsProductListingIngestionPublisher, SqsProductListingIngestionPublisher,
    with_publication_deadline,
};
use product_listing_service::use_cases::SubmitInternalProductListingIngestionHandler;
use serde_json::{Value, json};

use shopify_lambda::{ShopifyProductListingProcessor, handler, publication_deadline};
use std::{
    sync::{Arc, Mutex},
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::sync::{mpsc, oneshot};

const QUEUE_URL: &str = "https://sqs.us-east-1.amazonaws.com/123456789012/ingestion.fifo";

struct Source {
    listing_source_id: ListingSourceId,
    gate: Mutex<Option<(oneshot::Sender<()>, oneshot::Receiver<()>)>>,
}

#[async_trait::async_trait]
impl ShopifySourceReader for Source {
    async fn find_by_domain(
        &self,
        domain: &Domain,
    ) -> Result<Option<ShopifySource>, ListingSourceReadError> {
        let gate = self.gate.lock().unwrap().take();
        if let Some((entered, release)) = gate {
            entered.send(()).unwrap();
            release.await.unwrap();
        }
        Ok(Some(ShopifySource {
            listing_source_id: self.listing_source_id,
            domain: domain.clone(),
            currency: Some(money::Currency::Usd),
            language: None,
        }))
    }
}

#[derive(Clone, Debug)]
struct SqsHttp {
    requests: mpsc::UnboundedSender<Value>,
    stall_first: Arc<Mutex<bool>>,
}

impl HttpConnector for SqsHttp {
    fn call(&self, request: HttpRequest) -> HttpConnectorFuture {
        assert_eq!(request.method(), "POST");
        assert_eq!(request.uri(), "https://sqs.us-east-1.amazonaws.com/");
        assert_eq!(
            request.headers().get("x-amz-target"),
            Some("AmazonSQS.SendMessageBatch")
        );
        assert!(request.headers().get("authorization").is_some());
        let body: Value = serde_json::from_slice(request.body().bytes().unwrap()).unwrap();
        assert_eq!(body["QueueUrl"], QUEUE_URL);
        self.requests.send(body.clone()).unwrap();
        let stalled = std::mem::take(&mut *self.stall_first.lock().unwrap());
        if stalled {
            return HttpConnectorFuture::new(async {
                std::future::pending::<Result<HttpResponse, ConnectorError>>().await
            });
        }
        let successful: Vec<_> = body["Entries"].as_array().unwrap().iter().map(|entry| {
            json!({"Id": entry["Id"], "MessageId": "downstream-confirmed", "MD5OfMessageBody": "0123456789abcdef0123456789abcdef"})
        }).collect();
        let response: HttpResponse = http::Response::builder()
            .status(200)
            .header("content-type", "application/x-amz-json-1.0")
            .body(SdkBody::from(
                json!({"Successful": successful, "Failed": []}).to_string(),
            ))
            .unwrap()
            .try_into()
            .unwrap();
        HttpConnectorFuture::ready(Ok(response))
    }
}

impl HttpClient for SqsHttp {
    fn http_connector(
        &self,
        _: &HttpConnectorSettings,
        _: &RuntimeComponents,
    ) -> SharedHttpConnector {
        self.clone().into_shared()
    }
}

fn processor(
    source: Source,
    requests: mpsc::UnboundedSender<Value>,
    stall_first: bool,
) -> ShopifyProductListingProcessor<
    Source,
    SubmitInternalProductListingIngestionHandler<ScopedSqsProductListingIngestionPublisher>,
> {
    let config = aws_sdk_sqs::Config::builder()
        .behavior_version_latest()
        .region(Region::new("us-east-1"))
        .credentials_provider(Credentials::new(
            "test",
            "test",
            None,
            None,
            "shopify-deadline-test",
        ))
        .http_client(SqsHttp {
            requests,
            stall_first: Arc::new(Mutex::new(stall_first)),
        })
        .build();
    ShopifyProductListingProcessor::new(
        source,
        SubmitInternalProductListingIngestionHandler::new(
            ScopedSqsProductListingIngestionPublisher::new(
                SqsProductListingIngestionPublisher::new(
                    aws_sdk_sqs::Client::from_conf(config),
                    QUEUE_URL,
                ),
            ),
        ),
    )
}

fn event(message_id: &str, remaining: Duration) -> LambdaEvent<SqsEvent> {
    let body = json!({
        "version": "0", "id": "eventbridge-delivery", "detail-type": "shopifyWebhook",
        "source": "aws.partner/shopify.com/test", "account": "123456789012",
        "time": "2026-01-01T00:00:00Z", "region": "us-east-1", "resources": [],
        "detail": {
            "payload": {
                "id": 42, "title": "Cabinet", "handle": "cabinet", "status": "active",
                "variants": [{"price": "42.00", "inventory_quantity": 1}], "images": [],
                "futureShopifyKey": {"retained": true}
            },
            "metadata": {
                "X-Shopify-Topic": "products/create",
                "X-Shopify-Shop-Domain": "partner.example",
                "X-Shopify-Event-Id": "shopify-event"
            }
        }
    });
    let mut message = SqsMessage::default();
    message.message_id = Some(message_id.to_owned());
    message.body = Some(body.to_string());
    let mut context = Context::default();
    context.request_id = "shopify-test-invocation".to_owned();
    context.deadline = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64
        + remaining.as_millis() as u64;
    let mut sqs_event = SqsEvent::default();
    sqs_event.records = vec![message];
    LambdaEvent::new(sqs_event, context)
}

fn failures(response: SqsBatchResponse) -> Vec<String> {
    response
        .batch_item_failures
        .into_iter()
        .map(|failure| failure.item_identifier)
        .collect()
}

fn one_capture(request: Value) {
    let entries = request["Entries"].as_array().unwrap();
    assert_eq!(entries.len(), 1);
    let wire: Value = serde_json::from_str(entries[0]["MessageBody"].as_str().unwrap()).unwrap();
    assert_eq!(wire["payload"]["operation"], "CAPTURE_RAW");
    assert_eq!(wire["payload"]["command"]["sourceRecordKey"], "42");
    assert_eq!(
        wire["payload"]["command"]["input"]["sourcePayload"]["futureShopifyKey"]["retained"],
        true
    );
}

#[tokio::test(start_paused = true)]
async fn delayed_source_then_pending_sdk_send_retries_original_upstream_id_before_outer_budget() {
    let (entered_tx, entered_rx) = oneshot::channel();
    let (release_tx, release_rx) = oneshot::channel();
    let (requests_tx, mut requests_rx) = mpsc::unbounded_channel();
    let processor = processor(
        Source {
            listing_source_id: ListingSourceId::new(),
            gate: Mutex::new(Some((entered_tx, release_rx))),
        },
        requests_tx,
        true,
    );
    let event = event("upstream-delayed", Duration::from_secs(5));
    // Mirrors main: the outer publication scope is installed before credentials/source preparation.
    let deadline = publication_deadline(&event.context);
    let outer_timeout = tokio::time::Instant::now() + Duration::from_secs(4);
    let task = tokio::spawn(async move {
        with_publication_deadline(deadline, handler(event, &processor)).await
    });
    entered_rx.await.unwrap();
    tokio::time::advance(Duration::from_secs(2)).await;
    assert!(
        requests_rx.try_recv().is_err(),
        "no send during source lookup"
    );
    release_tx.send(()).unwrap();
    one_capture(requests_rx.recv().await.unwrap());
    assert!(!task.is_finished(), "SDK send is still pending");

    // Publication expires around 3.7s; the handler's outer timeout expires around 4s.
    tokio::time::advance(Duration::from_millis(1750)).await;
    assert_eq!(
        vec!["upstream-delayed"],
        failures(task.await.unwrap().unwrap())
    );
    assert!(
        tokio::time::Instant::now() < outer_timeout,
        "partial failure must be returned before the outer handler budget"
    );
    assert!(
        requests_rx.try_recv().is_err(),
        "no fresh send after publication deadline"
    );
}

#[tokio::test(start_paused = true)]
async fn expired_before_send_does_not_call_connector_or_poison_warm_next_invocation() {
    let (entered_tx, entered_rx) = oneshot::channel();
    let (release_tx, release_rx) = oneshot::channel();
    let (requests_tx, mut requests_rx) = mpsc::unbounded_channel();
    let processor = Arc::new(processor(
        Source {
            listing_source_id: ListingSourceId::new(),
            gate: Mutex::new(Some((entered_tx, release_rx))),
        },
        requests_tx,
        false,
    ));
    let expired = event("upstream-expired", Duration::from_secs(2));
    let deadline = publication_deadline(&expired.context);
    let outer_timeout = tokio::time::Instant::now() + Duration::from_secs(1);
    let running_processor = Arc::clone(&processor);
    let task = tokio::spawn(async move {
        with_publication_deadline(deadline, handler(expired, running_processor.as_ref())).await
    });
    entered_rx.await.unwrap();
    tokio::time::advance(Duration::from_millis(800)).await; // publication at ~700ms, outer at ~1s
    release_tx.send(()).unwrap();
    assert_eq!(
        vec!["upstream-expired"],
        failures(task.await.unwrap().unwrap())
    );
    assert!(
        tokio::time::Instant::now() < outer_timeout,
        "expired-before-send must report before handler timeout"
    );
    assert!(
        requests_rx.try_recv().is_err(),
        "expired message must not reach SQS"
    );

    let fresh = event("upstream-fresh", Duration::from_secs(5));
    let deadline = publication_deadline(&fresh.context);
    let result = with_publication_deadline(deadline, handler(fresh, processor.as_ref()))
        .await
        .unwrap();
    assert!(
        failures(result).is_empty(),
        "fresh invocation must confirm custody"
    );
    one_capture(requests_rx.recv().await.unwrap());
    assert!(requests_rx.try_recv().is_err());
}
