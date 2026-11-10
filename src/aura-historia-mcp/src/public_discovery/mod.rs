//! Public source reads with optional delegated context, and the anonymous overview.
mod errors;
mod sources;

use crate::{AccessClass, InvocationContext, OperationDefinition, OperationPolicy, RegistryError};
use application::operation_context::{OperationContext, Principal};
use aura_historia_contracts::public_overview::PublicOverviewData;
use aura_historia_runtime::{PublicListingSourceReadBudget, PublicListingSources, PublicOverview};
use schemars::JsonSchema;
use serde::Deserialize;

/// Family-local allowlist; release composition owns the production registry.
pub fn operations(
    sources: PublicListingSources,
    source_budget: PublicListingSourceReadBudget,
    overview: PublicOverview,
) -> Result<Vec<OperationDefinition>, RegistryError> {
    let mut operations = sources::operations(sources, source_budget)?;
    operations.push(overview_operation(overview)?);
    Ok(operations)
}

fn read_policy() -> OperationPolicy {
    OperationPolicy {
        access: AccessClass::Public,
        required_scopes: vec![],
        read_only: true,
        destructive: false,
        idempotent: true,
        open_world: false,
    }
}

fn anonymous_context(invocation: &InvocationContext) -> OperationContext {
    OperationContext {
        principal: Principal::Anonymous,
        ..invocation.operation.clone()
    }
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct OverviewArguments {}

fn overview_operation(services: PublicOverview) -> Result<OperationDefinition, RegistryError> {
    OperationDefinition::new(
        "getPublicOverview",
        "Get catalogue overview",
        "Get the latest calculated statistics of the Aura Historia catalogue: live listings, listings added in the last seven days, countries, languages and indexed providers by type, with the time they were calculated. Takes no arguments.",
        read_policy(),
        move |_: OverviewArguments, invocation| {
            let get = services.get.clone();
            async move {
                let context = anonymous_context(&invocation);
                let result = invocation
                    .execute(get.execute(&context))
                    .await?
                    .map_err(errors::overview)?;
                Ok(PublicOverviewData::from(result))
            }
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{MAX_RESPONSE_BYTES, OperationError, Registry};
    use application::error::static_error;
    use application::operation_context::{CorrelationId, RequestId};
    use rmcp::model::CallToolResult;
    use serde_json::{Value, json};
    use std::{sync::Arc, time::Duration};
    use tokio::time::Instant;
    use tokio_util::sync::CancellationToken;

    use fixtures::{Fixture, Outcome, Request, source_id};

    fn context() -> InvocationContext {
        InvocationContext {
            operation: OperationContext {
                principal: Principal::Anonymous,
                request_id: RequestId::new("discovery-test"),
                correlation_id: CorrelationId::new("discovery-correlation"),
            },
            deadline: Instant::now() + Duration::from_secs(5),
            cancellation: CancellationToken::new(),
            max_response_bytes: MAX_RESPONSE_BYTES,
        }
    }

    async fn call(registry: &Registry, name: &str, arguments: Value) -> CallToolResult {
        let operation = registry.get(name).expect("registered family operation");
        assert!(jsonschema::is_valid(
            &serde_json::to_value(&operation.tool().input_schema).unwrap(),
            &arguments
        ));
        let result = operation.call(arguments, context()).await.unwrap();
        let value = result.structured_content.as_ref().unwrap();
        assert!(jsonschema::is_valid(
            &serde_json::to_value(operation.tool().output_schema.as_ref().unwrap()).unwrap(),
            value
        ));
        result
    }

    async fn invalid(registry: &Registry, name: &str, arguments: Value) {
        let error = registry
            .get(name)
            .unwrap()
            .call(arguments, context())
            .await
            .unwrap_err();
        assert!(
            matches!(&error, OperationError::InvalidArguments(_)),
            "{error:?}"
        );
        assert_eq!(-32602, error.into_protocol().unwrap_err().code.0);
    }

    #[tokio::test]
    async fn should_preserve_browse_insufficient_text_and_search_with_query_bound_continuation() {
        let fixture = Arc::new(Fixture::default());
        let registry = fixture.registry(PublicListingSourceReadBudget::default());
        let browse = call(&registry, "searchPublicListingSources", json!({"size":1}))
            .await
            .structured_content
            .unwrap();
        assert_eq!(1, browse["size"]);
        let cursor = browse["searchAfter"].as_str().unwrap();
        let next = call(
            &registry,
            "searchPublicListingSources",
            json!({"size":1,"searchAfter":cursor}),
        )
        .await
        .structured_content
        .unwrap();
        assert!(next.get("searchAfter").is_none());
        invalid(
            &registry,
            "searchPublicListingSources",
            json!({"size":2,"searchAfter":cursor}),
        )
        .await;
        invalid(
            &registry,
            "searchPublicListingSources",
            json!({"query":"mu","size":1,"searchAfter":cursor}),
        )
        .await;
        invalid(
            &registry,
            "searchPublicListingSources",
            json!({"query":"a","size":1,"searchAfter":cursor}),
        )
        .await;
        let insufficient = call(
            &registry,
            "searchPublicListingSources",
            json!({"query":"a"}),
        )
        .await
        .structured_content
        .unwrap();
        assert_eq!(json!({"items":[],"size":0}), insufficient);
        let search = call(
            &registry,
            "searchPublicListingSources",
            json!({"query":"museum","size":50}),
        )
        .await
        .structured_content
        .unwrap();
        let cursor = search["searchAfter"].as_str().unwrap();
        call(
            &registry,
            "searchPublicListingSources",
            json!({"query":"  museum  ","size":50,"searchAfter":cursor}),
        )
        .await;
        invalid(
            &registry,
            "searchPublicListingSources",
            json!({"query":"other","size":50,"searchAfter":cursor}),
        )
        .await;
        let requests = fixture.requests.lock().unwrap();
        let Request::Search(context, request) = &requests[0] else {
            panic!("source search request")
        };
        assert_eq!(Principal::Anonymous, context.principal);
        assert_eq!("discovery-test", context.request_id.as_str());
        assert!(request.query().is_browse());
        assert_eq!(1, request.page_size());
        assert_eq!(
            5,
            requests.len(),
            "cursor mismatches must not call the use case"
        );
    }

    #[tokio::test]
    async fn should_reject_invalid_source_inputs_before_starting_service_work() {
        let fixture = Arc::new(Fixture::default());
        let registry = fixture.registry(PublicListingSourceReadBudget::default());
        for input in [
            json!({"size":0}),
            json!({"size":51}),
            json!({"size":"21"}),
            json!({"query":"x".repeat(257)}),
            json!({"query":"ab\u{0000}cd"}),
            json!({"searchAfter":"not-base64"}),
            json!({"searchAfter":"x".repeat(4097)}),
            json!({"privateGrant":true}),
        ] {
            invalid(&registry, "searchPublicListingSources", input).await;
        }
        for input in [
            json!({"listingSourceSlugId":"Museum"}),
            json!({"listingSourceSlugId":""}),
            json!({"listingSourceSlugId":"museum","contact":true}),
        ] {
            invalid(&registry, "getPublicListingSourceBySlug", input).await;
        }
        assert!(fixture.requests.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn should_preserve_all_public_source_fields_without_management_configuration() {
        let fixture = Arc::new(Fixture::default());
        let registry = fixture.registry(PublicListingSourceReadBudget::default());
        let detail = call(
            &registry,
            "getPublicListingSourceBySlug",
            json!({"listingSourceSlugId":"museum-source"}),
        )
        .await
        .structured_content
        .unwrap();
        assert_eq!(
            json!({"listingSourceId":source_id(),"listingSourceSlugId":"museum-source","name":"Museum Source","operator":{"name":"Museum Operator"},"url":"https://museum.example.test/","viewUrl":"https://museum.example.test/?utm_source=aura_historia&utm_medium=referral","image":"https://museum.example.test/card.jpg"}),
            detail
        );
        let search = call(&registry, "searchPublicListingSources", json!({}))
            .await
            .structured_content
            .unwrap();
        assert_eq!(detail, search["items"][0]);
        let requests = fixture.requests.lock().unwrap();
        let Request::Detail(context, request) = &requests[0] else {
            panic!("slug detail request")
        };
        assert_eq!(Principal::Anonymous, context.principal);
        assert_eq!("museum-source", request.slug_id.as_ref());
    }

    #[tokio::test]
    async fn should_return_every_public_overview_aggregate_anonymously() {
        let fixture = Arc::new(Fixture::default());
        let registry = fixture.registry(PublicListingSourceReadBudget::default());
        let result = call(&registry, "getPublicOverview", json!({}))
            .await
            .structured_content
            .unwrap();
        assert_eq!(
            json!({"asOf":"2026-10-19T12:34:56.789Z","sourcesIndexed":12,"sourcesIndexedByType":{"dealers":5,"auctionHouses":4,"marketplaces":2,"auctionPlatforms":1},"liveObjects":3456,"countries":7,"languages":5,"newObjectsLast7Days":89}),
            result
        );
        invalid(&registry, "getPublicOverview", json!({"language":"de"})).await;
        let requests = fixture.requests.lock().unwrap();
        let Request::Overview(context) = &requests[0] else {
            panic!("overview request")
        };
        assert_eq!(Principal::Anonymous, context.principal);
        assert_eq!("discovery-correlation", context.correlation_id.as_str());
    }

    #[tokio::test]
    async fn should_share_non_queuing_source_budget_and_release_it_after_timeout() {
        let fixture = Arc::new(Fixture {
            outcome: Outcome::Pending,
            ..Default::default()
        });
        let budget = PublicListingSourceReadBudget::new(1, Duration::from_millis(5));
        let registry = fixture.registry(budget.clone());
        let permit = budget.try_acquire().unwrap();
        for (name, arguments) in [
            ("searchPublicListingSources", json!({})),
            (
                "getPublicListingSourceBySlug",
                json!({"listingSourceSlugId":"museum-source"}),
            ),
        ] {
            let error = registry
                .get(name)
                .unwrap()
                .call(arguments, context())
                .await
                .unwrap_err();
            assert!(matches!(error, OperationError::Unavailable(_)));
        }
        assert!(fixture.requests.lock().unwrap().is_empty());
        drop(permit);
        for (name, arguments) in [
            ("searchPublicListingSources", json!({})),
            (
                "getPublicListingSourceBySlug",
                json!({"listingSourceSlugId":"museum-source"}),
            ),
        ] {
            let error = registry
                .get(name)
                .unwrap()
                .call(arguments, context())
                .await
                .unwrap_err();
            let result = error.into_protocol().unwrap();
            assert_eq!(Some(true), result.is_error);
            assert_eq!(
                "TEMPORARILY_UNAVAILABLE",
                result.content[0].as_text().unwrap().text
            );
            assert!(budget.try_acquire().is_some());
        }
    }

    #[tokio::test]
    async fn should_observe_invocation_deadline_and_cancellation_for_every_operation() {
        let fixture = Arc::new(Fixture {
            outcome: Outcome::Pending,
            ..Default::default()
        });
        let budget = PublicListingSourceReadBudget::new(1, Duration::from_secs(5));
        let registry = fixture.registry(budget.clone());
        for (name, arguments) in operation_arguments() {
            let mut expired = context();
            expired.deadline = Instant::now();
            let error = registry
                .get(name)
                .unwrap()
                .call(arguments.clone(), expired)
                .await
                .unwrap_err();
            assert!(matches!(error, OperationError::Unavailable(_)));
            let cancelled = context();
            cancelled.cancellation.cancel();
            let error = registry
                .get(name)
                .unwrap()
                .call(arguments, cancelled)
                .await
                .unwrap_err();
            assert!(matches!(error, OperationError::Unavailable(_)));
            assert!(budget.try_acquire().is_some());
        }
        assert!(fixture.requests.lock().unwrap().is_empty());
    }

    fn operation_arguments() -> [(&'static str, Value); 3] {
        [
            ("searchPublicListingSources", json!({})),
            (
                "getPublicListingSourceBySlug",
                json!({"listingSourceSlugId":"museum-source"}),
            ),
            ("getPublicOverview", json!({})),
        ]
    }

    #[tokio::test]
    async fn should_keep_service_failures_distinct_and_private() {
        for outcome in [Outcome::Unavailable, Outcome::Internal] {
            let fixture = Arc::new(Fixture {
                outcome,
                ..Default::default()
            });
            let registry = fixture.registry(PublicListingSourceReadBudget::default());
            for (name, arguments) in operation_arguments() {
                let error = registry
                    .get(name)
                    .unwrap()
                    .call(arguments, context())
                    .await
                    .unwrap_err();
                assert!(std::error::Error::source(&error).is_some());
                let response = match error.into_protocol() {
                    Ok(result) => serde_json::to_value(result).unwrap(),
                    Err(error) => serde_json::to_value(error).unwrap(),
                };
                assert!(!response.to_string().contains("private provider detail"));
                match outcome {
                    Outcome::Unavailable => assert_eq!(true, response["isError"]),
                    Outcome::Internal => assert_eq!(-32603, response["code"]),
                    _ => unreachable!(),
                }
            }
        }
        let fixture = Arc::new(Fixture {
            outcome: Outcome::Missing,
            ..Default::default()
        });
        let registry = fixture.registry(PublicListingSourceReadBudget::default());
        let missing = registry
            .get("getPublicListingSourceBySlug")
            .unwrap()
            .call(json!({"listingSourceSlugId":"missing"}), context())
            .await
            .unwrap_err();
        assert!(matches!(missing, OperationError::NotFound(_)));
        let missing = registry
            .get("getPublicOverview")
            .unwrap()
            .call(json!({}), context())
            .await
            .unwrap_err();
        assert!(matches!(missing, OperationError::Unavailable(_)));
    }

    #[tokio::test]
    async fn should_preserve_source_context_and_keep_overview_anonymous() {
        let fixture = Arc::new(Fixture::default());
        let registry = fixture.registry(PublicListingSourceReadBudget::default());
        for (name, arguments) in operation_arguments() {
            let mut invocation = context();
            invocation.operation.principal = Principal::System;
            registry
                .get(name)
                .unwrap()
                .call(arguments, invocation)
                .await
                .unwrap();
        }
        for request in fixture.requests.lock().unwrap().iter() {
            match request {
                Request::Search(context, _) | Request::Detail(context, _) => {
                    assert_eq!(Principal::System, context.principal);
                }
                Request::Overview(context) => assert_eq!(Principal::Anonymous, context.principal),
            }
        }
    }

    mod fixtures {
        use super::*;
        use application::transaction::{Transaction, TransactionError, UnitOfWork};
        use listing_source_core::{ListingSourceId, ListingSourceName, ListingSourceSlugId};
        use listing_source_service::{
            ports::{PublicListingSourceSearchReader, PublicListingSourceSearchReaderFactory},
            use_cases::queries::{
                get_public_listing_source_by_slug::{
                    GetPublicListingSourceBySlugError, GetPublicListingSourceBySlugRequest,
                    GetPublicListingSourceBySlugResult, GetPublicListingSourceBySlugUseCase,
                },
                public_listing_source::{
                    PublicListingSourceOperatorSummary, PublicListingSourceSummary,
                    PublicListingSourceWebsite,
                },
                search_public_listing_sources::{
                    PublicListingSourceSearchPage, PublicListingSourceSearchPosition,
                    SearchPublicListingSourcesError, SearchPublicListingSourcesHandler,
                    SearchPublicListingSourcesRequest, SearchPublicListingSourcesResult,
                    SearchPublicListingSourcesUseCase,
                },
            },
        };
        use party_core::party_name::PartyName;
        use product_listing_service::use_cases::{
            GetPublicCatalogueOverviewError, GetPublicCatalogueOverviewUseCase,
            PublicCatalogueOverview,
        };
        use std::sync::Mutex;

        #[derive(Clone, Copy, Default)]
        pub enum Outcome {
            #[default]
            Success,
            Pending,
            Unavailable,
            Internal,
            Missing,
        }

        #[derive(Debug)]
        pub enum Request {
            Search(OperationContext, SearchPublicListingSourcesRequest),
            Detail(OperationContext, GetPublicListingSourceBySlugRequest),
            Overview(OperationContext),
        }

        #[derive(Default)]
        pub struct Fixture {
            pub requests: Mutex<Vec<Request>>,
            pub outcome: Outcome,
        }

        impl Fixture {
            pub fn registry(self: &Arc<Self>, budget: PublicListingSourceReadBudget) -> Registry {
                Registry::new(
                    operations(
                        PublicListingSources {
                            search: self.clone(),
                            get_by_slug: self.clone(),
                        },
                        budget,
                        PublicOverview { get: self.clone() },
                    )
                    .unwrap(),
                )
                .unwrap()
            }

            fn record(&self, request: Request) {
                self.requests.lock().unwrap().push(request);
            }
        }

        #[async_trait::async_trait]
        impl SearchPublicListingSourcesUseCase for Fixture {
            async fn execute(
                &self,
                context: &OperationContext,
                request: SearchPublicListingSourcesRequest,
            ) -> Result<SearchPublicListingSourcesResult, SearchPublicListingSourcesError>
            {
                self.record(Request::Search(context.clone(), request.clone()));
                match self.outcome {
                    Outcome::Pending => std::future::pending().await,
                    Outcome::Unavailable => {
                        Err(SearchPublicListingSourcesError::TemporarilyUnavailable {
                            source: static_error("private provider detail"),
                        })
                    }
                    Outcome::Internal => Err(SearchPublicListingSourcesError::InvalidReadModel {
                        source: static_error("private provider detail"),
                    }),
                    _ => {
                        SearchPublicListingSourcesHandler::new(NoopUnitOfWork, SourceReader)
                            .execute(context, request)
                            .await
                    }
                }
            }
        }

        #[async_trait::async_trait]
        impl GetPublicListingSourceBySlugUseCase for Fixture {
            async fn execute(
                &self,
                context: &OperationContext,
                request: GetPublicListingSourceBySlugRequest,
            ) -> Result<GetPublicListingSourceBySlugResult, GetPublicListingSourceBySlugError>
            {
                self.record(Request::Detail(context.clone(), request.clone()));
                match self.outcome {
                    Outcome::Pending => std::future::pending().await,
                    Outcome::Unavailable => {
                        Err(GetPublicListingSourceBySlugError::TemporarilyUnavailable {
                            source: static_error("private provider detail"),
                        })
                    }
                    Outcome::Internal => Err(GetPublicListingSourceBySlugError::InvalidReadModel {
                        source: static_error("private provider detail"),
                    }),
                    Outcome::Missing => Err(GetPublicListingSourceBySlugError::NotFound),
                    Outcome::Success => Ok(PublicListingSourceSummary {
                        listing_source_slug_id: request.slug_id,
                        ..source()
                    }),
                }
            }
        }

        #[async_trait::async_trait]
        impl GetPublicCatalogueOverviewUseCase for Fixture {
            async fn execute(
                &self,
                context: &OperationContext,
            ) -> Result<PublicCatalogueOverview, GetPublicCatalogueOverviewError> {
                self.record(Request::Overview(context.clone()));
                match self.outcome {
                    Outcome::Pending => std::future::pending().await,
                    Outcome::Unavailable => Err(
                        GetPublicCatalogueOverviewError::ReaderTemporarilyUnavailable {
                            source: static_error("private provider detail"),
                        },
                    ),
                    Outcome::Internal => {
                        Err(GetPublicCatalogueOverviewError::ReaderInvalidReadModel {
                            source: static_error("private provider detail"),
                        })
                    }
                    Outcome::Missing => Err(GetPublicCatalogueOverviewError::NotYetCalculated),
                    Outcome::Success => Ok(PublicCatalogueOverview {
                        as_of: time::macros::datetime!(2026-10-19 12:34:56.789 UTC),
                        sources_indexed: 12,
                        sources_indexed_by_type:
                            product_listing_service::use_cases::SourcesIndexedByType {
                                dealers: 5,
                                auction_houses: 4,
                                marketplaces: 2,
                                auction_platforms: 1,
                            },
                        live_objects: 3456,
                        countries: 7,
                        languages: 5,
                        new_objects_last_7_days: 89,
                    }),
                }
            }
        }

        pub fn source_id() -> ListingSourceId {
            "ls_01h455vb4pex5vy7enb1p677vn".parse().unwrap()
        }

        fn source() -> PublicListingSourceSummary {
            PublicListingSourceSummary {
                listing_source_id: source_id(),
                listing_source_slug_id: ListingSourceSlugId::raw("museum-source").unwrap(),
                name: ListingSourceName::try_from("Museum Source").unwrap(),
                operator: PublicListingSourceOperatorSummary {
                    name: PartyName::try_from("Museum Operator").unwrap(),
                },
                website: Some(PublicListingSourceWebsite {
                    url: "https://museum.example.test/".parse().unwrap(),
                    view_url:
                        "https://museum.example.test/?utm_source=aura_historia&utm_medium=referral"
                            .parse()
                            .unwrap(),
                }),
                image: Some("https://museum.example.test/card.jpg".parse().unwrap()),
            }
        }

        struct NoopUnitOfWork;
        struct NoopTransaction;
        #[async_trait::async_trait]
        impl UnitOfWork for NoopUnitOfWork {
            type Tx = NoopTransaction;
            async fn begin(&self) -> Result<Self::Tx, TransactionError> {
                Ok(NoopTransaction)
            }
        }
        #[async_trait::async_trait]
        impl Transaction for NoopTransaction {
            async fn commit(self) -> Result<(), TransactionError> {
                Ok(())
            }
        }

        #[derive(Clone, Copy)]
        struct SourceReader;
        impl PublicListingSourceSearchReaderFactory<NoopTransaction> for SourceReader {
            fn in_transaction<'tx>(
                &'tx self,
                _: &'tx mut NoopTransaction,
            ) -> impl PublicListingSourceSearchReader + 'tx {
                *self
            }
        }
        #[async_trait::async_trait]
        impl PublicListingSourceSearchReader for SourceReader {
            async fn search(
                &mut self,
                request: &SearchPublicListingSourcesRequest,
            ) -> Result<
                PublicListingSourceSearchPage,
                listing_source_service::ports::PublicListingSourceSearchReadError,
            > {
                Ok(PublicListingSourceSearchPage {
                    items: vec![source()],
                    next_position: request.continuation().is_none().then(|| {
                        PublicListingSourceSearchPosition::new(
                            if request.query().is_browse() { 0 } else { 1 },
                            "museum source".into(),
                            source_id(),
                        )
                        .unwrap()
                    }),
                })
            }
        }
    }
}
