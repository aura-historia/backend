use super::*;
use crate::{InvocationContext, MAX_RESPONSE_BYTES, Registry};
use application::operation_context::{CorrelationId, OperationContext, Principal, RequestId};
use application::patch_field::PatchField;
use partnership_service::use_cases::commands::put_listing_source_ingestion_configuration::*;
use product_listing_service::{
    ports::{ProductListingIngestionPublishError, ProductListingIngestionPublisher},
    use_cases::{
        ProductListingIngestionItemOutcome, ProductListingIngestionMessage,
        ProductListingIngestionNotAttemptedReason as NotAttempted,
        ProductListingIngestionOutcome as Outcome,
        ProductListingIngestionRejectionReason as Rejection,
        SubmitPartnerProductListingIngestionHandler,
    },
};
use serde_json::{Value, json};
use std::{collections::BTreeSet, sync::Mutex};
use tokio_util::sync::CancellationToken;
#[path = "tests/postgres.rs"]
mod postgres;

const NAMES: [&str; 4] = [
    "postAsyncPartnerProductListings",
    "patchAsyncPartnerProductListings",
    "putAsyncPartnerProductListings",
    "deleteAsyncPartnerProductListings",
];
#[derive(Clone, Default)]
struct Publisher {
    sent: Arc<Mutex<Vec<Vec<ProductListingIngestionMessage>>>>,
}
#[async_trait::async_trait]
impl ProductListingIngestionPublisher for Publisher {
    async fn publish(
        &self,
        messages: Vec<ProductListingIngestionMessage>,
    ) -> Result<Vec<ProductListingIngestionItemOutcome>, ProductListingIngestionPublishError> {
        self.sent.lock().unwrap().push(messages.clone());
        Ok(messages
            .into_iter()
            .map(|message| {
                let id = message.intent.source_listing_id().unwrap().as_ref();
                let outcome = match id {
                    "unknown" => Outcome::Unconfirmed,
                    "oversize" => Outcome::Rejected {
                        reason: Rejection::Publisher {
                            code: "INVALID_MESSAGE_SIZE".into(),
                        },
                        retryable: false,
                    },
                    "not-attempted" => Outcome::NotAttempted {
                        reason: NotAttempted::DeadlineExceeded,
                        retryable: true,
                    },
                    "blocked" => Outcome::NotAttempted {
                        reason: NotAttempted::BlockedByFifoPredecessor,
                        retryable: false,
                    },
                    _ => Outcome::Accepted,
                };
                ProductListingIngestionItemOutcome {
                    index: message.metadata.index,
                    command_id: message.metadata.command_id,
                    outcome,
                }
            })
            .collect())
    }
}
#[derive(Default)]
struct Configuration {
    calls: Mutex<Vec<(Principal, PutListingSourceIngestionConfigurationCommand)>>,
}
#[async_trait::async_trait]
impl PutListingSourceIngestionConfigurationUseCase for Configuration {
    async fn execute(
        &self,
        context: &OperationContext,
        command: PutListingSourceIngestionConfigurationCommand,
    ) -> Result<
        PutListingSourceIngestionConfigurationResult,
        PutListingSourceIngestionConfigurationError,
    > {
        self.calls
            .lock()
            .unwrap()
            .push((context.principal.clone(), command));
        Ok(PutListingSourceIngestionConfigurationResult { created: true })
    }
}
fn context() -> InvocationContext {
    InvocationContext {
        operation: OperationContext {
            principal: Principal::DelegatedUser {
                user_id: "usr_01h455vb4pex5vy7enb1p677vn".parse().unwrap(),
                capabilities: BTreeSet::from([
                    CredentialCapability::ProductListingsWrite,
                    CredentialCapability::ListingSourcesWrite,
                ]),
            },
            request_id: RequestId::new("ingestion-test"),
            correlation_id: CorrelationId::new("ingestion-test"),
        },
        deadline: tokio::time::Instant::now() + Duration::from_secs(10),
        cancellation: CancellationToken::new(),
        max_response_bytes: MAX_RESPONSE_BYTES,
    }
}
fn fixture() -> (Registry, Publisher, Arc<Configuration>) {
    let publisher = Publisher::default();
    let configuration = Arc::new(Configuration::default());
    let registry = Registry::new(
        operations(PartnerProductListingIngestion {
            submit: Arc::new(SubmitPartnerProductListingIngestionHandler::new(
                publisher.clone(),
            )),
            put_configuration: configuration.clone(),
        })
        .unwrap(),
    )
    .unwrap();
    (registry, publisher, configuration)
}
fn product(name: &str, id: &str) -> Value {
    let mut product = json!({"sourceListingId":id,"title":{"text":"Cabinet","language":"en"},"description":{"text":"Schrank","language":"de"},"price":{"type":"MONETARY","currency":"EUR","amount":u64::MAX},"priceEstimateMin":{"currency":"USD","amount":0},"priceEstimateMax":{"currency":"GBP","amount":u64::MAX},"availability":"AVAILABLE","url":"https://source.test/listing","images":["https://source.test/one.jpg","https://source.test/two.jpg"],"auction":{"auctionId":null,"lotNumber":"Lot 42","cataloguePosition":u32::MAX,"timing":{"biddingOpens":null,"scheduledCloses":"2026-10-23T12:00:00Z","reportedClosedAt":null}}});
    if name == NAMES[1] {
        product.as_object_mut().unwrap().remove("title");
        product.as_object_mut().unwrap().remove("description");
    }
    if name == NAMES[3] {
        product = json!({"sourceListingId":id,"legacyIgnored":true});
    }
    product
}
async fn call(
    registry: &Registry,
    name: &str,
    input: Value,
    context: InvocationContext,
) -> Result<Value, OperationError> {
    let op = registry.get(name).unwrap();
    let result = op.call(input, context).await?.structured_content.unwrap();
    let schema = serde_json::to_value(&**op.tool().output_schema.as_ref().unwrap()).unwrap();
    assert!(
        jsonschema::validator_for(&schema)
            .unwrap()
            .is_valid(&result),
        "output schema: {result}"
    );
    Ok(result)
}

#[tokio::test]
async fn should_expose_only_five_allowed_operations_and_preserve_full_inputs() {
    let (registry, publisher, configuration) = fixture();
    let source = ListingSourceId::new();
    for name in NAMES {
        let op = registry.get(name).unwrap();
        assert!(!op.policy().idempotent);
        assert_eq!(
            vec![CredentialCapability::ProductListingsWrite],
            op.policy().required_scopes
        );
        let input = json!({"listingSourceId":source,"idempotencyKey":"full","products":[product(name,"  full  ")]});
        let schema = serde_json::to_value(&*op.tool().input_schema).unwrap();
        assert!(jsonschema::validator_for(&schema).unwrap().is_valid(&input));
        let report = call(&registry, name, input, context()).await.unwrap();
        assert_eq!(report["acceptedCount"], 1);
        assert_eq!(report["idempotencyKey"], "full");
        assert_eq!(report["items"][0]["outcome"], "PUBLISHED");
        assert_eq!(report["items"][0]["sourceListingId"], "full");
    }
    {
        let sent = publisher.sent.lock().unwrap();
        let ProductListingIngestionIntent::Create(command) = &sent[0][0].intent else {
            panic!("create")
        };
        assert_eq!(source, command.listing_source_id);
        assert_eq!(2, command.images.len());
        assert_eq!("Cabinet", command.title.as_ref().unwrap().payload.as_ref());
        assert_eq!(
            localization::Language::De,
            command.description.as_ref().unwrap().localization
        );
        assert_eq!(
            u64::MAX,
            u64::from(command.pricing.price_estimate_max.unwrap().monetary_amount)
        );
        assert_eq!(
            PatchField::Clear,
            command.auction.as_ref().unwrap().auction_id
        );
        let ProductListingIngestionIntent::Update { command, .. } = &sent[1][0].intent else {
            panic!("update")
        };
        assert!(matches!(command.price, PatchField::Set(_)));
        assert!(matches!(command.images,PatchField::Set(ref images) if images.len()==2));
        assert!(matches!(command.auction, PatchField::Set(_)));
        let ProductListingIngestionIntent::Upsert(command) = &sent[2][0].intent else {
            panic!("upsert")
        };
        assert!(matches!(command.images,PatchField::Set(ref images) if images.len()==2));
        assert!(matches!(
            sent[3][0].intent,
            ProductListingIngestionIntent::Withdraw(_)
        ));
    }
    let result=call(&registry,"putShopifyListingSourceIngestionConfiguration",json!({"listingSourceId":source,"configuration":{"domain":"shop.example.test","currency":"EUR","language":"de"}}),context()).await.unwrap();
    assert_eq!(result, json!({"created":true}));
    let config = configuration.calls.lock().unwrap();
    assert!(matches!(config[0].0, Principal::DelegatedUser { .. }));
    let listing_source_service::ports::ListingIngestionConfiguration::Shopify {
        domain,
        currency,
        language,
    } = &config[0].1.configuration
    else {
        panic!("shopify")
    };
    assert_eq!("shop.example.test", domain.as_str());
    assert_eq!(&Some(money::Currency::Eur), currency);
    assert_eq!(&Some(localization::Language::De), language);
    assert!(
        registry
            .get("putWoocommerceListingSourceIngestionConfiguration")
            .is_none()
    );
    assert_eq!(5, registry.page(None).unwrap().0.len());
}

#[tokio::test]
async fn should_preserve_original_positions_and_distinguish_all_admission_outcomes() {
    let (registry, publisher, _) = fixture();
    let source = ListingSourceId::new();
    let report=call(&registry,NAMES[1],json!({"listingSourceId":source,"idempotencyKey":"mixed","products":[{"sourceListingId":"okay"},null,{"sourceListingId":"unknown"},{"sourceListingId":"oversize"},{"sourceListingId":"not-attempted"},{"sourceListingId":"blocked"},{"sourceListingId":"bad","images":null}]}),context()).await.unwrap();
    assert_eq!(report["originalInputCount"], 7);
    assert_eq!(report["acceptedCount"], 1);
    let outcomes = report["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item["outcome"].as_str().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(
        outcomes,
        vec![
            "PUBLISHED",
            "REJECTED",
            "UNKNOWN",
            "REJECTED",
            "NOT_ATTEMPTED",
            "NOT_ATTEMPTED",
            "REJECTED"
        ]
    );
    assert_eq!(report["items"][2]["error"], "ENQUEUE_UNCONFIRMED");
    assert_eq!(report["items"][2]["retryable"], true);
    assert_eq!(
        report["items"][3]["error"],
        "PRODUCT_LISTING_INGESTION_PAYLOAD_TOO_LARGE"
    );
    assert_eq!(report["items"][3]["retryable"], false);
    assert_eq!(report["items"][4]["error"], "ENQUEUE_NOT_ATTEMPTED");
    assert_eq!(report["items"][5]["error"], "ENQUEUE_BLOCKED");
    assert_eq!(report["items"][5]["retryable"], false);
    assert_eq!(
        publisher.sent.lock().unwrap()[0]
            .iter()
            .map(|message| message.metadata.index)
            .collect::<Vec<_>>(),
        vec![0, 2, 3, 4, 5]
    );
    assert!(
        publisher.sent.lock().unwrap()[0]
            .iter()
            .all(|message| message.metadata.input_count == 7)
    );
}

#[tokio::test]
async fn should_preserve_retry_identity_and_return_generated_effective_keys() {
    let (registry, publisher, _) = fixture();
    let source = ListingSourceId::new();
    let input = json!({"listingSourceId":source,"idempotencyKey":"stable","products":[{"sourceListingId":"same"},null,{"sourceListingId":"same"}]});
    let ctx = context();
    let first = call(&registry, NAMES[1], input.clone(), ctx.clone())
        .await
        .unwrap();
    let second = call(&registry, NAMES[1], input, ctx.clone()).await.unwrap();
    assert_eq!(first, second);
    {
        let sent = publisher.sent.lock().unwrap();
        assert_eq!(sent[0], sent[1]);
        assert_ne!(
            sent[0][0].metadata.command_id,
            sent[0][1].metadata.command_id
        );
    }
    let mut other_source = source;
    while other_source == source {
        other_source = ListingSourceId::new();
    }
    let other=call(&registry,NAMES[1],json!({"listingSourceId":other_source,"idempotencyKey":"stable","products":[{"sourceListingId":"same"},null,{"sourceListingId":"same"}]}),ctx.clone()).await.unwrap();
    assert_ne!(first["submissionId"], other["submissionId"]);
    let generated = call(
        &registry,
        NAMES[1],
        json!({"listingSourceId":source,"products":[]}),
        ctx.clone(),
    )
    .await
    .unwrap();
    assert!(
        generated["idempotencyKey"]
            .as_str()
            .unwrap()
            .starts_with("generated-")
    );
    let replay=call(&registry,NAMES[1],json!({"listingSourceId":source,"idempotencyKey":generated["idempotencyKey"],"products":[]}),ctx).await.unwrap();
    assert_eq!(generated, replay);
}

#[tokio::test]
async fn should_reject_envelopes_keys_and_missing_capabilities_before_publication() {
    let (registry, publisher, configuration) = fixture();
    let source = ListingSourceId::new();
    for key in ["", "with space", "comma,key", "\u{7f}", &"a".repeat(129)] {
        let error=call(&registry,NAMES[1],json!({"listingSourceId":source,"idempotencyKey":key,"products":[{"sourceListingId":"okay"}]}),context()).await.unwrap_err();
        assert!(matches!(error, OperationError::InvalidArguments(_)));
    }
    let error = call(
        &registry,
        NAMES[1],
        json!({"listingSourceId":source,"products":vec![json!(null);101]}),
        context(),
    )
    .await
    .unwrap_err();
    assert!(matches!(error, OperationError::InvalidArguments(_)));
    let mut ctx = context();
    ctx.operation.principal = Principal::DelegatedUser {
        user_id: "usr_01h455vb4pex5vy7enb1p677vn".parse().unwrap(),
        capabilities: BTreeSet::new(),
    };
    assert!(matches!(
        call(
            &registry,
            NAMES[1],
            json!({"listingSourceId":source,"products":[{"sourceListingId":"okay"}]}),
            ctx
        )
        .await,
        Err(OperationError::Forbidden)
    ));
    for config in [
        json!({"domain":"https://invalid.test"}),
        json!({"domain":"shop.example.test","currency":"XYZ"}),
        json!({"domain":"shop.example.test","language":"xx"}),
        json!({"domain":"shop.example.test","webhookSecret":"private"}),
    ] {
        assert!(matches!(
            call(
                &registry,
                "putShopifyListingSourceIngestionConfiguration",
                json!({"listingSourceId":source,"configuration":config}),
                context()
            )
            .await,
            Err(OperationError::InvalidArguments(_))
        ));
    }
    assert!(publisher.sent.lock().unwrap().is_empty());
    assert!(configuration.calls.lock().unwrap().is_empty());
}

#[derive(Clone)]
struct Gate;
#[async_trait::async_trait]
impl crate::AdmissionGate for Gate {
    async fn admit(
        &self,
        _: crate::AdmissionRequest<'_>,
    ) -> Result<Principal, crate::AdmissionRejection> {
        Ok(context().operation.principal)
    }
}
async fn raw_http(registry: Registry, name: &str, arguments: &str) -> Value {
    use axum::{
        body::{Body, to_bytes},
        http::{Request, StatusCode},
    };
    use tower::ServiceExt;
    let app = crate::router(
        crate::TransportConfig {
            allowed_hosts: vec!["mcp.example.test".into()],
            ..Default::default()
        },
        registry,
        Arc::new(Gate),
    )
    .unwrap();
    let body = format!(
        r#"{{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{{"name":"{name}","arguments":{arguments}}}}}"#
    );
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1")
                .header("host", "mcp.example.test")
                .header("content-type", "application/json")
                .header("accept", "application/json, text/event-stream")
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(StatusCode::OK, response.status());
    serde_json::from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap()).unwrap()
}
#[tokio::test]
async fn should_reject_only_the_later_duplicate_field_item_through_raw_http_for_each_verb() {
    for name in NAMES {
        let (registry, publisher, _) = fixture();
        let source = ListingSourceId::new();
        let valid = product(name, "same");
        let duplicate = valid
            .to_string()
            .replacen("{", r#"{"sourceListingId":"ambiguous","#, 1);
        let arguments = format!(
            r#"{{"listingSourceId":"{source}","idempotencyKey":"raw","products":[{valid},{duplicate},{valid}]}}"#
        );
        let response = raw_http(registry, name, &arguments).await;
        assert_eq!(
            response["result"]["structuredContent"]["acceptedCount"], 2,
            "{response}"
        );
        assert_eq!(
            response["result"]["structuredContent"]["originalInputCount"],
            3
        );
        assert_eq!(
            response["result"]["structuredContent"]["failures"],
            json!([{"index":1,"error":"BAD_BODY_VALUE","retryable":false}])
        );
        assert_eq!(
            publisher.sent.lock().unwrap()[0]
                .iter()
                .map(|m| m.metadata.index)
                .collect::<Vec<_>>(),
            vec![0, 2]
        );
    }
}

#[tokio::test]
async fn should_reserve_response_time_and_scope_the_reusable_publisher_per_request() {
    use aura_historia_runtime::{ProductListingIngestionDependencies, Runtime};
    let pool = sqlx::postgres::PgPoolOptions::new()
        .connect_lazy("postgres://fixture:fixture@127.0.0.1:9/fixture")
        .unwrap();
    let runtime = Runtime::from_pool(pool);
    let config = aws_sdk_sqs::config::Builder::new()
        .behavior_version_latest()
        .region(aws_sdk_sqs::config::Region::new("eu-central-1"))
        .credentials_provider(aws_sdk_sqs::config::Credentials::new(
            "fixture", "fixture", None, None, "fixture",
        ))
        .build();
    let services = runtime
        .partner_product_listing_ingestion(ProductListingIngestionDependencies::from_sqs(
            aws_sdk_sqs::Client::from_conf(config),
            "http://127.0.0.1:9/fixture.fifo".into(),
        ))
        .clone();
    let registry = Registry::new(operations(services).unwrap()).unwrap();
    for key in ["first", "second"] {
        let mut ctx = context();
        ctx.deadline = tokio::time::Instant::now() + Duration::from_millis(250);
        let deadline = ctx.deadline;
        let result=call(&registry,NAMES[1],json!({"listingSourceId":ListingSourceId::new(),"idempotencyKey":key,"products":[{"sourceListingId":"unsent"}]}),ctx).await.unwrap();
        assert_eq!(result["acceptedCount"], 0);
        assert_eq!(result["items"][0]["outcome"], "NOT_ATTEMPTED");
        assert_eq!(result["items"][0]["error"], "ENQUEUE_NOT_ATTEMPTED");
        assert!(tokio::time::Instant::now() < deadline);
    }
}
