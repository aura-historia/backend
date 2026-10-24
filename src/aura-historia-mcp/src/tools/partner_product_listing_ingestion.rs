//! Source-keyed queue admission and non-secret Shopify configuration.
pub(super) mod error;
use crate::{AccessClass, OperationDefinition, OperationError, OperationPolicy, RegistryError};
use application::operation_context::CredentialCapability;
use aura_historia_contracts::{
    listing_sources::shopify::PutShopifyConfigurationData,
    partner_product_listings::{
        CreateProductListingData, PartnerProductListingInput, UpdateProductListingData,
        UpsertProductListingData, WithdrawProductListingData,
        admission::{self, AdmissionReportData, RawAdmissionItems},
    },
    product_listing_management::MutationInputError,
};
use aura_historia_runtime::{PartnerProductListingIngestion, with_publication_deadline};
use listing_source_core::ListingSourceId;
use product_listing_service::use_cases::{
    ProductListingIngestionIntent, ProductListingIngestionSubmission,
    SubmitPartnerProductListingIngestionUseCase,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use std::{sync::Arc, time::Duration};

const RESPONSE_RESERVE: Duration = Duration::from_millis(500);
const ADMISSION_DESCRIPTION: &str = "Submit up to 100 source-keyed listings for FIFO queue admission. Invalid items are rejected independently and original positions are retained. PUBLISHED confirms queue custody; source grants and business rules are checked when the worker applies each command. REJECTED, NOT_ATTEMPTED and UNKNOWN are distinct; admission never confirms a committed listing mutation. submissionId is correlation, not a status resource. Optional idempotencyKey accepts 1-128 visible ASCII bytes excluding comma; omission generates a key returned in the report, unrecoverable after a lost reply. Retry a partial or lost reply only with the same actor, source, effective key and complete unchanged ordered batch, retaining content, operations, original indices and originalInputCount. No automatic retries or exactly-once guarantee.";

#[derive(Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields, bound(deserialize = ""))]
struct BatchInput<T> {
    #[schemars(schema_with = "aura_historia_contracts::wire::schema::listing_source_id")]
    listing_source_id: ListingSourceId,
    #[serde(default)]
    #[schemars(schema_with = "admission::optional_idempotency_key_schema")]
    idempotency_key: Option<String>,
    products: RawAdmissionItems<T>,
}
#[derive(Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ShopifyInput {
    #[schemars(schema_with = "aura_historia_contracts::wire::schema::listing_source_id")]
    listing_source_id: ListingSourceId,
    configuration: PutShopifyConfigurationData,
}
#[derive(Serialize, JsonSchema)]
struct ShopifyResult {
    created: bool,
}
fn policy(
    capability: CredentialCapability,
    destructive: bool,
    idempotent: bool,
) -> OperationPolicy {
    OperationPolicy {
        access: AccessClass::Protected,
        required_scopes: vec![capability],
        read_only: false,
        destructive,
        idempotent,
        open_world: false,
    }
}

/// Family only; partner release composition publishes the combined catalogue.
pub fn operations(
    services: PartnerProductListingIngestion,
) -> Result<Vec<OperationDefinition>, RegistryError> {
    let mut operations = vec![
        batch_operation(
            services.submit.clone(),
            "postAsyncPartnerProductListings",
            false,
            |data: CreateProductListingData, source| {
                data.into_command(source)
                    .map(ProductListingIngestionIntent::Create)
            },
        )?,
        batch_operation(
            services.submit.clone(),
            "patchAsyncPartnerProductListings",
            true,
            |data: UpdateProductListingData, source| {
                data.into_key_and_command(source)
                    .map(
                        |(product_key, command)| ProductListingIngestionIntent::Update {
                            product_key,
                            command,
                        },
                    )
            },
        )?,
        batch_operation(
            services.submit.clone(),
            "putAsyncPartnerProductListings",
            true,
            |data: UpsertProductListingData, source| {
                data.into_command(source)
                    .map(ProductListingIngestionIntent::Upsert)
            },
        )?,
        batch_operation(
            services.submit,
            "deleteAsyncPartnerProductListings",
            true,
            |data: WithdrawProductListingData, source| {
                data.into_product_key(source)
                    .map(ProductListingIngestionIntent::Withdraw)
            },
        )?,
    ];
    operations.push(OperationDefinition::new(
        "putShopifyListingSourceIngestionConfiguration",
        "Replace the complete non-secret Shopify domain, optional currency and language configuration. Missing or null currency/language clears the provider override. Uses current source grants and listing-sources:write; created reports whether Shopify was newly configured. This commits source configuration synchronously.",
        policy(CredentialCapability::ListingSourcesWrite, true, true),
        move |input: ShopifyInput, context| {
            let service = services.put_configuration.clone();
            async move {
                let configuration = input.configuration.into_configuration().map_err(|error| OperationError::InvalidArguments(Box::new(error)))?;
                let result = context.execute(service.execute(&context.operation, partnership_service::use_cases::commands::put_listing_source_ingestion_configuration::PutListingSourceIngestionConfigurationCommand { listing_source_id: input.listing_source_id, configuration })).await?.map_err(error::configuration)?;
                Ok(ShopifyResult { created: result.created })
            }
        },
    )?);
    Ok(operations)
}

fn batch_operation<T, F>(
    service: Arc<dyn SubmitPartnerProductListingIngestionUseCase>,
    name: &'static str,
    destructive: bool,
    into_intent: F,
) -> Result<OperationDefinition, RegistryError>
where
    T: PartnerProductListingInput + DeserializeOwned + JsonSchema + Send + 'static,
    F: Fn(T, ListingSourceId) -> Result<ProductListingIngestionIntent, MutationInputError>
        + Send
        + Sync
        + 'static,
{
    let into_intent = Arc::new(into_intent);
    OperationDefinition::new(
        name,
        ADMISSION_DESCRIPTION,
        policy(
            CredentialCapability::ProductListingsWrite,
            destructive,
            false,
        ),
        move |input: BatchInput<T>, context| {
            let service = service.clone();
            let into_intent = into_intent.clone();
            async move {
                let key = input
                    .idempotency_key
                    .as_deref()
                    .map(admission::parse_idempotency_key)
                    .transpose()
                    .map_err(|error| OperationError::InvalidArguments(Box::new(error)))?;
                let raw = input.products.into_raw();
                let count = raw.len();
                // DTOs remain the sole item decoder. Only their raw representation differs from sync batches.
                let (items, source_ids, failures) = admission::parse_items::<T>(
                    raw,
                    |data| data.source_listing_id(),
                    |data| into_intent(data, input.listing_source_id),
                );
                let deadline = context
                    .deadline
                    .checked_sub(RESPONSE_RESERVE)
                    .unwrap_or_else(tokio::time::Instant::now);
                let result = context
                    .execute(with_publication_deadline(
                        deadline,
                        service.execute(
                            &context.operation,
                            ProductListingIngestionSubmission {
                                listing_source_id: input.listing_source_id,
                                original_input_count: count,
                                idempotency_key: key,
                                items,
                            },
                        ),
                    ))
                    .await?
                    .map_err(error::submission)?;
                Ok::<AdmissionReportData, OperationError>(admission::report(
                    result,
                    failures,
                    &source_ids,
                ))
            }
        },
    )
}

#[cfg(test)]
mod tests {
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
        ) -> Result<Vec<ProductListingIngestionItemOutcome>, ProductListingIngestionPublishError>
        {
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
    async fn should_return_complete_validated_source_ids_at_domain_limits_for_each_verb() {
        let submitted_ids = [
            "a".repeat(128),
            "b".repeat(129),
            "é".repeat(256),
            format!("{}trimmed", " ".repeat(129)),
            "line\nbreak".into(),
        ];
        for name in NAMES {
            let (registry, publisher, _) = fixture();
            let mut products = submitted_ids
                .iter()
                .map(|id| product(name, id))
                .collect::<Vec<_>>();
            products.push(product(name, &"é".repeat(257)));
            let report = call(
                &registry,
                name,
                json!({"listingSourceId":ListingSourceId::new(),"products":products}),
                context(),
            )
            .await
            .unwrap();
            assert_eq!(report["acceptedCount"], submitted_ids.len());
            for (index, id) in submitted_ids.iter().enumerate() {
                assert_eq!(report["items"][index]["sourceListingId"], id.trim());
                assert_eq!(report["items"][index]["outcome"], "PUBLISHED");
            }
            assert_eq!(report["items"][5]["outcome"], "REJECTED");
            assert!(report["items"][5].get("sourceListingId").is_none());
            assert_eq!(publisher.sent.lock().unwrap()[0].len(), submitted_ids.len());
        }
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
    async fn should_match_key_schema_and_runtime_for_omission_null_values_and_invalid_keys() {
        for name in NAMES {
            let (registry, publisher, _) = fixture();
            let source = ListingSourceId::new();
            let schema =
                serde_json::to_value(&*registry.get(name).unwrap().tool().input_schema).unwrap();
            let validator = jsonschema::validator_for(&schema).unwrap();
            let base = json!({"listingSourceId":source,"products":[product(name,"schema-key")]});
            for key in [None, Some(Value::Null), Some(json!("explicit-key!"))] {
                let mut input = base.clone();
                if let Some(key) = &key {
                    input["idempotencyKey"] = key.clone();
                }
                assert!(validator.is_valid(&input), "{name}: {input}");
                let report = call(&registry, name, input, context()).await.unwrap();
                assert_eq!(report["acceptedCount"], 1);
                match key {
                    Some(Value::String(key)) => assert_eq!(report["idempotencyKey"], key),
                    _ => assert!(
                        report["idempotencyKey"]
                            .as_str()
                            .unwrap()
                            .starts_with("generated-")
                    ),
                }
            }
            assert_eq!(publisher.sent.lock().unwrap().len(), 3);
            for key in [
                json!(""),
                json!("space key"),
                json!("comma,key"),
                json!("é"),
                json!("a".repeat(129)),
                json!(42),
            ] {
                let mut input = base.clone();
                input["idempotencyKey"] = key;
                assert!(!validator.is_valid(&input), "{name}: {input}");
                assert!(matches!(
                    call(&registry, name, input, context()).await,
                    Err(OperationError::InvalidArguments(_))
                ));
            }
            assert_eq!(publisher.sent.lock().unwrap().len(), 3);
        }
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

    #[tokio::test]
    async fn should_match_shopify_schema_and_serde_for_omitted_null_and_populated_overrides() {
        let (registry, _, configuration) = fixture();
        let name = "putShopifyListingSourceIngestionConfiguration";
        let schema =
            serde_json::to_value(&*registry.get(name).unwrap().tool().input_schema).unwrap();
        let validator = jsonschema::validator_for(&schema).unwrap();
        for (overrides, expected_currency, expected_language) in [
            (json!({}), None, None),
            (json!({"currency":null,"language":null}), None, None),
            (
                json!({"currency":"EUR","language":"de"}),
                Some(money::Currency::Eur),
                Some(localization::Language::De),
            ),
        ] {
            let mut data = json!({"domain":"shop.example.test"});
            data.as_object_mut()
                .unwrap()
                .extend(overrides.as_object().unwrap().clone());
            let input = json!({"listingSourceId":ListingSourceId::new(),"configuration":data});
            assert!(validator.is_valid(&input), "{input}");
            call(&registry, name, input, context()).await.unwrap();
            let calls = configuration.calls.lock().unwrap();
            let listing_source_service::ports::ListingIngestionConfiguration::Shopify {
                currency,
                language,
                ..
            } = &calls.last().unwrap().1.configuration
            else {
                panic!("Shopify configuration")
            };
            assert_eq!(*currency, expected_currency);
            assert_eq!(*language, expected_language);
        }
        for data in [
            json!({}),
            json!({"domain":null}),
            json!({"domain":"shop.example.test","currency":1}),
            json!({"domain":"shop.example.test","language":false}),
            json!({"domain":"shop.example.test","webhookSecret":"private"}),
        ] {
            let input = json!({"listingSourceId":ListingSourceId::new(),"configuration":data});
            assert!(!validator.is_valid(&input), "{input}");
            assert!(matches!(
                call(&registry, name, input, context()).await,
                Err(OperationError::InvalidArguments(_))
            ));
        }
        assert_eq!(configuration.calls.lock().unwrap().len(), 3);
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
            let duplicate =
                valid
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

    mod postgres {
        use super::*;
        use aura_historia_runtime::{ProductListingIngestionDependencies, Runtime};
        use test_api::{IntegrationTestService, aura_integration_test, get_postgres_client};
        const SCHEMA: test_api::Postgres = test_api::Postgres::new("migrations");
        async fn seed_source(pool: &sqlx::PgPool) -> (uuid::Uuid, ListingSourceId) {
            let party = uuid::Uuid::now_v7();
            let source = ListingSourceId::new();
            sqlx::query(
                "INSERT INTO parties(party_id,party_slug_id,name) VALUES($1,$2,'Operator')",
            )
            .bind(party)
            .bind(format!("party-{party}"))
            .execute(pool)
            .await
            .unwrap();
            sqlx::query("INSERT INTO listing_sources(listing_source_id,listing_source_slug_id,name,operator_party_id) VALUES($1,$2,'Partner source',$3)").bind(source.into_uuid()).bind(format!("source-{}",source.as_uuid())).bind(party).execute(pool).await.unwrap();
            (party, source)
        }
        async fn grant_source(
            pool: &sqlx::PgPool,
            user_id: user_core::user_id::UserId,
            party: uuid::Uuid,
            source: ListingSourceId,
        ) {
            let partnership = uuid::Uuid::now_v7();
            sqlx::query("INSERT INTO partnerships(partnership_id,party_id) VALUES($1,$2)")
                .bind(partnership)
                .bind(party)
                .execute(pool)
                .await
                .unwrap();
            sqlx::query("INSERT INTO partnership_members(partnership_id,user_id) VALUES($1,$2)")
                .bind(partnership)
                .bind(user_id.into_uuid())
                .execute(pool)
                .await
                .unwrap();
            sqlx::query("INSERT INTO partnership_listing_source_grants(partnership_id,listing_source_id) VALUES($1,$2)").bind(partnership).bind(source.into_uuid()).execute(pool).await.unwrap();
        }

        #[aura_integration_test(services=[SCHEMA], isolated=true)]
        async fn should_check_shopify_source_grants_synchronously_and_keep_admission_separate_from_commit()
         {
            let pool = get_postgres_client().await;
            let ctx = context();
            let Principal::DelegatedUser { user_id, .. } = ctx.operation.principal else {
                panic!("delegated actor")
            };
            sqlx::query("INSERT INTO users(user_id,email,tier,role) VALUES($1,$2,'FREE','USER')")
                .bind(user_id.into_uuid())
                .bind(format!("{user_id}@example.test"))
                .execute(&pool)
                .await
                .unwrap();
            let (party, source) = seed_source(&pool).await;
            let runtime = Runtime::from_pool(pool.clone());
            let publisher = Publisher::default();
            let registry = Registry::new(
                operations(
                    runtime
                        .partner_product_listing_ingestion(
                            ProductListingIngestionDependencies::new(publisher.clone()),
                        )
                        .clone(),
                )
                .unwrap(),
            )
            .unwrap();
            let input = json!({"listingSourceId":source,"configuration":{"domain":"shop.example.test","currency":"EUR","language":"de"}});
            assert!(matches!(
                call(
                    &registry,
                    "putShopifyListingSourceIngestionConfiguration",
                    input.clone(),
                    ctx.clone()
                )
                .await,
                Err(OperationError::Forbidden)
            ));
            grant_source(&pool, user_id, party, source).await;
            let created = call(
                &registry,
                "putShopifyListingSourceIngestionConfiguration",
                input.clone(),
                ctx.clone(),
            )
            .await
            .unwrap();
            assert_eq!(created, json!({"created":true}));
            let stored:(String,Option<String>,Option<String>)=sqlx::query_as("SELECT domain,currency,language FROM listing_source_shopify_ingestion_configurations WHERE listing_source_id=$1").bind(source.into_uuid()).fetch_one(&pool).await.unwrap();
            assert_eq!(
                stored,
                (
                    "shop.example.test".into(),
                    Some("EUR".into()),
                    Some("de".into())
                )
            );
            let replaced = call(
                &registry,
                "putShopifyListingSourceIngestionConfiguration",
                json!({"listingSourceId":source,"configuration":{"domain":"shop.example.test"}}),
                ctx.clone(),
            )
            .await
            .unwrap();
            assert_eq!(replaced, json!({"created":false}));
            let cleared:(Option<String>,Option<String>)=sqlx::query_as("SELECT currency,language FROM listing_source_shopify_ingestion_configurations WHERE listing_source_id=$1").bind(source.into_uuid()).fetch_one(&pool).await.unwrap();
            assert_eq!(cleared, (None, None));
            let mut unscoped = ctx.clone();
            unscoped.operation.principal = Principal::DelegatedUser {
                user_id,
                capabilities: BTreeSet::from([CredentialCapability::ProductListingsWrite]),
            };
            assert!(matches!(
                call(
                    &registry,
                    "putShopifyListingSourceIngestionConfiguration",
                    input.clone(),
                    unscoped
                )
                .await,
                Err(OperationError::Forbidden)
            ));
            sqlx::query("DELETE FROM partnership_listing_source_grants WHERE listing_source_id=$1")
                .bind(source.into_uuid())
                .execute(&pool)
                .await
                .unwrap();
            assert!(matches!(
                call(
                    &registry,
                    "putShopifyListingSourceIngestionConfiguration",
                    input,
                    ctx.clone()
                )
                .await,
                Err(OperationError::Forbidden)
            ));
            // Admission intentionally does not look up grants or write canonical listings.
            let report=call(&registry,NAMES[0],json!({"listingSourceId":source,"idempotencyKey":"admit-with-revoked-grant","products":[product(NAMES[0],"queued")]}),ctx).await.unwrap();
            assert_eq!(report["acceptedCount"], 1);
            assert_eq!(1, publisher.sent.lock().unwrap().len());
            let count: i64 = sqlx::query_scalar(
                "SELECT count(*) FROM product_listings WHERE listing_source_id=$1",
            )
            .bind(source.into_uuid())
            .fetch_one(&pool)
            .await
            .unwrap();
            assert_eq!(0, count);
        }
    }
}
