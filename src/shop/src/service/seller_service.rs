use crate::core::partner_status::ShopPartnerStatus;
use crate::core::shop::Shop;
use crate::core::shop_search::ShopSearch;
use crate::core::shop_type::ShopType;
use crate::dynamodb::raw_shop_name_record::{self, RawShopNameRecord};
use crate::dynamodb::repository::ShopDynamoDbRepository;
use crate::service::command::CreateShopCommand;
use crate::service::command_service::CommandShopService;
use crate::service::get_service::GetShopService;
use crate::service::query_service::QueryShopService;
use common::logging::{
    LlmInvocationMetrics, LlmModel, LlmOperation, LlmProvider, log_llm_invocation,
};
use common::{query::text_query::TextQuery, shop_id::ShopId, shop_name::ShopName, slug_id::SlugId};
use llm::{LLMProvider, chat::ChatMessage};
use std::time::Instant;
use time::OffsetDateTime;
use tracing::info;

#[derive(Debug, thiserror::Error)]
pub enum SellerServiceError {
    #[error("LLM error: {0}")]
    LLMError(#[from] llm::error::LLMError),

    #[error("LLM returned no text response when disambiguating seller shop")]
    LLMNoTextResponse,

    #[error("Command shop error: {0}")]
    CommandShopError(#[from] crate::service::command_service::CommandShopError),

    #[error("Search shops error: {0}")]
    SearchShopsError(#[from] crate::service::query_service::SearchShopsError),

    #[error("DynamoDB GetItem error: {0}")]
    SdkGetItemError(
        #[from]
        aws_sdk_dynamodb::error::SdkError<aws_sdk_dynamodb::operation::get_item::GetItemError>,
    ),

    #[error("DynamoDB PutItem error: {0}")]
    SdkPutItemError(
        #[from]
        aws_sdk_dynamodb::error::SdkError<
            aws_sdk_dynamodb::operation::put_item::PutItemError,
            aws_sdk_dynamodb::config::http::HttpResponse,
        >,
    ),

    #[error("Seller shop details unexpectedly unavailable after creation attempt")]
    UnexpectedNone,
}

#[async_trait::async_trait]
#[mockall::automock]
pub trait SellerService {
    async fn find_seller_shop_details(
        &self,
        raw_shop_name: &ShopName,
    ) -> Result<Option<(ShopId, SlugId<0>, ShopName)>, SellerServiceError>;

    async fn create_seller_shop_details(
        &self,
        raw_shop_name: &ShopName,
    ) -> Result<Option<(ShopId, SlugId<0>, ShopName)>, SellerServiceError>;

    async fn get_seller_shop_details(
        &self,
        raw_shop_name: &ShopName,
    ) -> Result<(ShopId, SlugId<0>, ShopName), SellerServiceError>;
}

pub struct SellerServiceImpl<'a> {
    repository: &'a (dyn ShopDynamoDbRepository + Sync),
    #[allow(dead_code)]
    get_shop_service: &'a (dyn GetShopService + Sync),
    query_shop_service: &'a (dyn QueryShopService + Sync),
    command_shop_service: &'a (dyn CommandShopService + Sync),
    llm: Box<dyn LLMProvider>,
}

impl<'a> SellerServiceImpl<'a> {
    pub fn new(
        repository: &'a (dyn ShopDynamoDbRepository + Sync),
        get_shop_service: &'a (dyn GetShopService + Sync),
        query_shop_service: &'a (dyn QueryShopService + Sync),
        command_shop_service: &'a (dyn CommandShopService + Sync),
        llm: llm::builder::LLMBuilder,
    ) -> Result<Self, llm::error::LLMError> {
        let system_prompt = "You are a shop-name disambiguation assistant for an antiques auction platform.\n\
            Given a raw scraped shop name and up to three candidate shops from the database, \
            determine whether any candidate is highly likely to be the same shop as the raw name.\n\n\
            Response format — choose EXACTLY ONE:\n\
              MATCH:<index>   — if you are highly confident (index is 0, 1, or 2)\n\
              NONE            — if no candidate is a confident match\n\n\
            Rules:\n\
              - Output a SINGLE LINE with no extra text, explanation, or formatting.\n\
              - Only respond MATCH if confidence is very high (>90%). Prefer NONE when uncertain.";
        let llm = llm
            .resilient(true)
            .resilient_attempts(3)
            .system(system_prompt)
            .openai_enable_web_search(false)
            .reasoning(false)
            .timeout_seconds(30)
            .build()?;
        Ok(Self {
            repository,
            get_shop_service,
            query_shop_service,
            command_shop_service,
            llm,
        })
    }

    async fn disambiguate_with_llm(
        &self,
        raw_shop_name: &ShopName,
        candidates: &[Shop],
    ) -> Result<Option<Shop>, SellerServiceError> {
        let candidates_text = candidates
            .iter()
            .enumerate()
            .map(|(i, s)| format!("{i}: {}", s.name))
            .collect::<Vec<_>>()
            .join("\n");

        let prompt = format!(
            "Raw shop name: \"{raw_shop_name}\"\n\nCandidate shops:\n{candidates_text}\n\n\
             Respond with MATCH:<index> or NONE.",
        );
        let message = ChatMessage::user().content(prompt).build();

        let started_at = Instant::now();
        let response = self.llm.chat(&[message]).await?;
        log_llm_invocation(
            LlmOperation::SellerShopDisambiguation,
            LlmProvider::Configured,
            LlmModel::Configured,
            started_at.elapsed(),
            llm_metrics(response.usage()),
        );
        let response = response
            .text()
            .ok_or(SellerServiceError::LLMNoTextResponse)?;

        let response = response.trim();
        if let Some(idx_str) = response.strip_prefix("MATCH:")
            && let Ok(idx) = idx_str.trim().parse::<usize>()
            && let Some(shop) = candidates.get(idx)
        {
            return Ok(Some(shop.clone()));
        }
        Ok(None)
    }
}

fn llm_metrics(usage: Option<llm::chat::Usage>) -> LlmInvocationMetrics {
    let Some(usage) = usage else {
        return LlmInvocationMetrics::default();
    };

    LlmInvocationMetrics {
        batch_size: Some(1),
        prompt_tokens: Some(usage.prompt_tokens),
        completion_tokens: Some(usage.completion_tokens),
        total_tokens: Some(usage.total_tokens),
        cached_prompt_tokens: usage.prompt_tokens_details.and_then(|d| d.cached_tokens),
        reasoning_tokens: usage
            .completion_tokens_details
            .and_then(|d| d.reasoning_tokens),
        ..Default::default()
    }
}

#[async_trait::async_trait]
impl<'a> SellerService for SellerServiceImpl<'a> {
    async fn find_seller_shop_details(
        &self,
        raw_shop_name: &ShopName,
    ) -> Result<Option<(ShopId, SlugId<0>, ShopName)>, SellerServiceError> {
        let record = self
            .repository
            .get_raw_shop_name_record(raw_shop_name)
            .await?;
        Ok(record.map(|r| (r.shop_id, r.shop_slug_id, r.name)))
    }

    async fn create_seller_shop_details(
        &self,
        raw_shop_name: &ShopName,
    ) -> Result<Option<(ShopId, SlugId<0>, ShopName)>, SellerServiceError> {
        let search = ShopSearch {
            shop_name_query: Some(
                TextQuery::try_from(raw_shop_name.as_ref())
                    .expect("ShopName always fits TextQuery<0>"),
            ),
            ..Default::default()
        };

        let search_result = self
            .query_shop_service
            .search_shops(&search, &None, &None)
            .await?;

        let candidates: Vec<Shop> = search_result.items.into_iter().take(3).collect();

        let shop = if !candidates.is_empty() {
            match self
                .disambiguate_with_llm(raw_shop_name, &candidates)
                .await?
            {
                Some(matched_shop) => {
                    info!(
                        shopId = %matched_shop.shop_id,
                        name = %matched_shop.name,
                        rawName = %raw_shop_name,
                        "LLM matched raw shop name to existing shop."
                    );
                    matched_shop
                }
                None => {
                    self.command_shop_service
                        .create(CreateShopCommand {
                            name: raw_shop_name.clone(),
                            shop_type: ShopType::AuctionHouse,
                            shop_partner_status: ShopPartnerStatus::Scraped,
                            domains: Default::default(),
                            shopify_domain: None,
                            shopify_currency: None,
                            shopify_language: None,
                            woocommerce_webhook_secret: None,
                            woocommerce_currency: None,
                            woocommerce_language: None,
                            url: None,
                            image: None,
                            structured_address: None,
                            phone: None,
                            email: None,
                            affiliate_configuration: None,
                        })
                        .await?
                }
            }
        } else {
            self.command_shop_service
                .create(CreateShopCommand {
                    name: raw_shop_name.clone(),
                    shop_type: ShopType::AuctionHouse,
                    shop_partner_status: ShopPartnerStatus::Scraped,
                    domains: Default::default(),
                    shopify_domain: None,
                    shopify_currency: None,
                    shopify_language: None,
                    woocommerce_webhook_secret: None,
                    woocommerce_currency: None,
                    woocommerce_language: None,
                    url: None,
                    image: None,
                    structured_address: None,
                    phone: None,
                    email: None,
                    affiliate_configuration: None,
                })
                .await?
        };

        let record = RawShopNameRecord {
            pk: raw_shop_name_record::mk_pk(raw_shop_name),
            sk: raw_shop_name_record::mk_sk().to_owned(),
            raw_name: raw_shop_name.clone(),
            shop_id: shop.shop_id,
            shop_slug_id: shop.shop_slug_id.clone(),
            name: shop.name.clone(),
            created: OffsetDateTime::now_utc(),
            updated: OffsetDateTime::now_utc(),
        };
        let _ = self.repository.put_raw_shop_name_record(record).await?;

        Ok(Some((shop.shop_id, shop.shop_slug_id, shop.name)))
    }

    async fn get_seller_shop_details(
        &self,
        raw_shop_name: &ShopName,
    ) -> Result<(ShopId, SlugId<0>, ShopName), SellerServiceError> {
        if let Some(details) = self.find_seller_shop_details(raw_shop_name).await? {
            return Ok(details);
        }
        self.create_seller_shop_details(raw_shop_name)
            .await?
            .ok_or(SellerServiceError::UnexpectedNone)
    }
}

#[cfg(test)]
mod tests {
    use crate::core::shop::Shop;
    use crate::dynamodb::raw_shop_name_record::RawShopNameRecord;
    use crate::dynamodb::repository::MockShopDynamoDbRepository;
    use crate::service::command_service::{CommandShopError, MockCommandShopService};
    use crate::service::get_service::MockGetShopService;
    use crate::service::query_service::{MockQueryShopService, SearchShopsError};
    use crate::service::seller_service::{SellerService, SellerServiceError, SellerServiceImpl};
    use aws_sdk_dynamodb::{
        config::http::HttpResponse,
        error::{ConnectorError, SdkError},
        operation::put_item::PutItemOutput,
    };
    use common::{
        pagination::cursor::{Cursor, CursoredResult},
        shop_name::ShopName,
    };
    use fake::{Fake, Faker};
    use llm::{LLMProvider, chat::ChatMessage, error::LLMError};

    // ── LLM test infrastructure ──────────────────────────────────────────────

    struct FakeChatResponse(Option<String>);

    impl std::fmt::Display for FakeChatResponse {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            match &self.0 {
                Some(text) => write!(f, "{text}"),
                None => write!(f, ""),
            }
        }
    }

    impl std::fmt::Debug for FakeChatResponse {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(f, "FakeChatResponse({:?})", self.0)
        }
    }

    impl llm::chat::ChatResponse for FakeChatResponse {
        fn text(&self) -> Option<String> {
            self.0.clone()
        }

        fn tool_calls(&self) -> Option<Vec<llm::ToolCall>> {
            None
        }
    }

    /// A mock LLM provider that panics if called — used when we expect no LLM
    /// interaction.
    struct MockLlmProvider;

    #[async_trait::async_trait]
    impl llm::chat::ChatProvider for MockLlmProvider {
        async fn chat_with_tools(
            &self,
            _messages: &[ChatMessage],
            _tools: Option<&[llm::chat::Tool]>,
        ) -> Result<Box<dyn llm::chat::ChatResponse>, LLMError> {
            panic!("LLM should not be called in this test")
        }
    }

    #[async_trait::async_trait]
    impl llm::completion::CompletionProvider for MockLlmProvider {
        async fn complete(
            &self,
            _req: &llm::completion::CompletionRequest,
        ) -> Result<llm::completion::CompletionResponse, LLMError> {
            unimplemented!()
        }
    }

    #[async_trait::async_trait]
    impl llm::embedding::EmbeddingProvider for MockLlmProvider {
        async fn embed(&self, _input: Vec<String>) -> Result<Vec<Vec<f32>>, LLMError> {
            unimplemented!()
        }
    }

    #[async_trait::async_trait]
    impl llm::stt::SpeechToTextProvider for MockLlmProvider {
        async fn transcribe(&self, _audio: Vec<u8>) -> Result<String, LLMError> {
            unimplemented!()
        }
    }

    #[async_trait::async_trait]
    impl llm::tts::TextToSpeechProvider for MockLlmProvider {}

    #[async_trait::async_trait]
    impl llm::models::ModelsProvider for MockLlmProvider {}

    impl LLMProvider for MockLlmProvider {}

    /// A mock LLM provider that returns a fixed text response.
    struct MockLlmProviderReturning(String);

    #[async_trait::async_trait]
    impl llm::chat::ChatProvider for MockLlmProviderReturning {
        async fn chat_with_tools(
            &self,
            _messages: &[ChatMessage],
            _tools: Option<&[llm::chat::Tool]>,
        ) -> Result<Box<dyn llm::chat::ChatResponse>, LLMError> {
            Ok(Box::new(FakeChatResponse(Some(self.0.clone()))))
        }
    }

    #[async_trait::async_trait]
    impl llm::completion::CompletionProvider for MockLlmProviderReturning {
        async fn complete(
            &self,
            _req: &llm::completion::CompletionRequest,
        ) -> Result<llm::completion::CompletionResponse, LLMError> {
            unimplemented!()
        }
    }

    #[async_trait::async_trait]
    impl llm::embedding::EmbeddingProvider for MockLlmProviderReturning {
        async fn embed(&self, _input: Vec<String>) -> Result<Vec<Vec<f32>>, LLMError> {
            unimplemented!()
        }
    }

    #[async_trait::async_trait]
    impl llm::stt::SpeechToTextProvider for MockLlmProviderReturning {
        async fn transcribe(&self, _audio: Vec<u8>) -> Result<String, LLMError> {
            unimplemented!()
        }
    }

    #[async_trait::async_trait]
    impl llm::tts::TextToSpeechProvider for MockLlmProviderReturning {}

    #[async_trait::async_trait]
    impl llm::models::ModelsProvider for MockLlmProviderReturning {}

    impl LLMProvider for MockLlmProviderReturning {}

    // ── find_seller_shop_details ─────────────────────────────────────────────

    #[tokio::test]
    async fn should_return_some_when_raw_shop_name_record_exists_for_find_seller_shop_details() {
        let record: RawShopNameRecord = Faker.fake();
        let expected_shop_id = record.shop_id;
        let expected_slug = record.shop_slug_id.clone();
        let expected_name = record.name.clone();

        let mut repository = MockShopDynamoDbRepository::default();
        repository
            .expect_get_raw_shop_name_record()
            .return_once(move |_| Box::pin(async move { Ok(Some(record)) }));

        let get_shop_service = MockGetShopService::default();
        let query_shop_service = MockQueryShopService::default();
        let command_shop_service = MockCommandShopService::default();

        let service = SellerServiceImpl {
            repository: &repository,
            get_shop_service: &get_shop_service,
            query_shop_service: &query_shop_service,
            command_shop_service: &command_shop_service,
            llm: Box::new(MockLlmProvider),
        };

        let raw_name: ShopName = Faker.fake();
        let result = service.find_seller_shop_details(&raw_name).await.unwrap();

        assert!(result.is_some());
        let (shop_id, slug, name) = result.unwrap();
        assert_eq!(expected_shop_id, shop_id);
        assert_eq!(expected_slug, slug);
        assert_eq!(expected_name, name);
    }

    #[tokio::test]
    async fn should_return_none_when_raw_shop_name_record_not_exists_for_find_seller_shop_details()
    {
        let mut repository = MockShopDynamoDbRepository::default();
        repository
            .expect_get_raw_shop_name_record()
            .return_once(|_| Box::pin(async { Ok(None) }));

        let get_shop_service = MockGetShopService::default();
        let query_shop_service = MockQueryShopService::default();
        let command_shop_service = MockCommandShopService::default();

        let service = SellerServiceImpl {
            repository: &repository,
            get_shop_service: &get_shop_service,
            query_shop_service: &query_shop_service,
            command_shop_service: &command_shop_service,
            llm: Box::new(MockLlmProvider),
        };

        let raw_name: ShopName = Faker.fake();
        let result = service.find_seller_shop_details(&raw_name).await.unwrap();

        assert!(result.is_none());
    }

    #[tokio::test]
    #[rstest::rstest]
    #[case::construction_failure(SdkError::construction_failure("Something went wrong"))]
    #[case::timeout(SdkError::timeout_error("Something went wrong"))]
    #[case::dispatch_failure(SdkError::dispatch_failure(ConnectorError::user(
        "Something went wrong".into()
    )))]
    #[case::response_error(SdkError::response_error(
        "Something went wrong",
        HttpResponse::new(500u16.try_into().unwrap(), "{}".into())
    ))]
    #[case::service_error(SdkError::service_error(
        aws_sdk_dynamodb::operation::get_item::GetItemError::unhandled("Something went wrong"),
        HttpResponse::new(500u16.try_into().unwrap(), "{}".into())
    ))]
    #[trace]
    async fn should_propagate_sdk_error_for_find_seller_shop_details(
        #[case] expected: SdkError<aws_sdk_dynamodb::operation::get_item::GetItemError>,
    ) {
        let mut repository = MockShopDynamoDbRepository::default();
        repository
            .expect_get_raw_shop_name_record()
            .return_once(|_| Box::pin(async { Err(expected) }));

        let get_shop_service = MockGetShopService::default();
        let query_shop_service = MockQueryShopService::default();
        let command_shop_service = MockCommandShopService::default();

        let service = SellerServiceImpl {
            repository: &repository,
            get_shop_service: &get_shop_service,
            query_shop_service: &query_shop_service,
            command_shop_service: &command_shop_service,
            llm: Box::new(MockLlmProvider),
        };

        let raw_name: ShopName = Faker.fake();
        let result = service.find_seller_shop_details(&raw_name).await;

        assert!(result.is_err());
        match result.unwrap_err() {
            SellerServiceError::SdkGetItemError(_) => {}
            _ => panic!("expected SellerServiceError::SdkGetItemError"),
        }
    }

    // ── create_seller_shop_details ───────────────────────────────────────────

    #[tokio::test]
    async fn should_create_new_shop_when_no_candidates_found_for_create_seller_shop_details() {
        let shop: Shop = Faker.fake();
        let expected_shop_id = shop.shop_id;
        let expected_slug = shop.shop_slug_id.clone();
        let expected_name = shop.name.clone();

        let mut repository = MockShopDynamoDbRepository::default();
        repository
            .expect_put_raw_shop_name_record()
            .return_once(|_| Box::pin(async { Ok(PutItemOutput::builder().build()) }));

        let get_shop_service = MockGetShopService::default();

        let mut query_shop_service = MockQueryShopService::default();
        query_shop_service
            .expect_search_shops()
            .return_once(|_, _, _| {
                Box::pin(async {
                    Ok(CursoredResult {
                        items: vec![],
                        cursor: Cursor {
                            size: 0,
                            search_after: None,
                        },
                        total: Some(0),
                    })
                })
            });

        let mut command_shop_service = MockCommandShopService::default();
        command_shop_service
            .expect_create()
            .return_once(move |_| Box::pin(async move { Ok(shop) }));

        let service = SellerServiceImpl {
            repository: &repository,
            get_shop_service: &get_shop_service,
            query_shop_service: &query_shop_service,
            command_shop_service: &command_shop_service,
            llm: Box::new(MockLlmProvider),
        };

        let raw_name: ShopName = Faker.fake();
        let result = service.create_seller_shop_details(&raw_name).await.unwrap();

        assert!(result.is_some());
        let (shop_id, slug, name) = result.unwrap();
        assert_eq!(expected_shop_id, shop_id);
        assert_eq!(expected_slug, slug);
        assert_eq!(expected_name, name);
    }

    #[tokio::test]
    async fn should_use_existing_shop_when_llm_matches_candidate_for_create_seller_shop_details() {
        let shop: Shop = Faker.fake();
        let expected_shop_id = shop.shop_id;
        let expected_slug = shop.shop_slug_id.clone();
        let expected_name = shop.name.clone();

        let mut repository = MockShopDynamoDbRepository::default();
        repository
            .expect_put_raw_shop_name_record()
            .return_once(|_| Box::pin(async { Ok(PutItemOutput::builder().build()) }));

        let get_shop_service = MockGetShopService::default();

        let shops = vec![shop];
        let mut query_shop_service = MockQueryShopService::default();
        query_shop_service
            .expect_search_shops()
            .return_once(move |_, _, _| {
                Box::pin(async move {
                    Ok(CursoredResult {
                        items: shops,
                        cursor: Cursor {
                            size: 1,
                            search_after: None,
                        },
                        total: Some(1),
                    })
                })
            });

        // command_shop_service.create should NOT be called
        let command_shop_service = MockCommandShopService::default();

        let service = SellerServiceImpl {
            repository: &repository,
            get_shop_service: &get_shop_service,
            query_shop_service: &query_shop_service,
            command_shop_service: &command_shop_service,
            llm: Box::new(MockLlmProviderReturning("MATCH:0".to_string())),
        };

        let raw_name: ShopName = Faker.fake();
        let result = service.create_seller_shop_details(&raw_name).await.unwrap();

        assert!(result.is_some());
        let (shop_id, slug, name) = result.unwrap();
        assert_eq!(expected_shop_id, shop_id);
        assert_eq!(expected_slug, slug);
        assert_eq!(expected_name, name);
    }

    #[tokio::test]
    async fn should_create_new_shop_when_llm_returns_none_for_create_seller_shop_details() {
        let candidate: Shop = Faker.fake();
        let new_shop: Shop = Faker.fake();
        let expected_shop_id = new_shop.shop_id;

        let mut repository = MockShopDynamoDbRepository::default();
        repository
            .expect_put_raw_shop_name_record()
            .return_once(|_| Box::pin(async { Ok(PutItemOutput::builder().build()) }));

        let get_shop_service = MockGetShopService::default();

        let mut query_shop_service = MockQueryShopService::default();
        query_shop_service
            .expect_search_shops()
            .return_once(move |_, _, _| {
                Box::pin(async move {
                    Ok(CursoredResult {
                        items: vec![candidate],
                        cursor: Cursor {
                            size: 1,
                            search_after: None,
                        },
                        total: Some(1),
                    })
                })
            });

        let mut command_shop_service = MockCommandShopService::default();
        command_shop_service
            .expect_create()
            .return_once(move |_| Box::pin(async move { Ok(new_shop) }));

        let service = SellerServiceImpl {
            repository: &repository,
            get_shop_service: &get_shop_service,
            query_shop_service: &query_shop_service,
            command_shop_service: &command_shop_service,
            llm: Box::new(MockLlmProviderReturning("NONE".to_string())),
        };

        let raw_name: ShopName = Faker.fake();
        let result = service.create_seller_shop_details(&raw_name).await.unwrap();

        assert!(result.is_some());
        let (shop_id, _, _) = result.unwrap();
        assert_eq!(expected_shop_id, shop_id);
    }

    #[tokio::test]
    async fn should_propagate_search_error_for_create_seller_shop_details() {
        use serde::ser::Error as _;

        let repository = MockShopDynamoDbRepository::default();
        let get_shop_service = MockGetShopService::default();

        let mut query_shop_service = MockQueryShopService::default();
        query_shop_service
            .expect_search_shops()
            .return_once(|_, _, _| {
                Box::pin(async {
                    Err(SearchShopsError::OpenSearchError(opensearch::Error::from(
                        serde_json::Error::custom("simulated search error"),
                    )))
                })
            });

        let command_shop_service = MockCommandShopService::default();

        let service = SellerServiceImpl {
            repository: &repository,
            get_shop_service: &get_shop_service,
            query_shop_service: &query_shop_service,
            command_shop_service: &command_shop_service,
            llm: Box::new(MockLlmProvider),
        };

        let raw_name: ShopName = Faker.fake();
        let result = service.create_seller_shop_details(&raw_name).await;

        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            SellerServiceError::SearchShopsError(_)
        ));
    }

    #[tokio::test]
    async fn should_propagate_llm_error_for_create_seller_shop_details() {
        struct AlwaysErrorLlmProvider;

        #[async_trait::async_trait]
        impl llm::chat::ChatProvider for AlwaysErrorLlmProvider {
            async fn chat_with_tools(
                &self,
                _messages: &[ChatMessage],
                _tools: Option<&[llm::chat::Tool]>,
            ) -> Result<Box<dyn llm::chat::ChatResponse>, LLMError> {
                Err(LLMError::ProviderError("simulated LLM failure".to_string()))
            }
        }

        #[async_trait::async_trait]
        impl llm::completion::CompletionProvider for AlwaysErrorLlmProvider {
            async fn complete(
                &self,
                _req: &llm::completion::CompletionRequest,
            ) -> Result<llm::completion::CompletionResponse, LLMError> {
                unimplemented!()
            }
        }

        #[async_trait::async_trait]
        impl llm::embedding::EmbeddingProvider for AlwaysErrorLlmProvider {
            async fn embed(&self, _input: Vec<String>) -> Result<Vec<Vec<f32>>, LLMError> {
                unimplemented!()
            }
        }

        #[async_trait::async_trait]
        impl llm::stt::SpeechToTextProvider for AlwaysErrorLlmProvider {
            async fn transcribe(&self, _audio: Vec<u8>) -> Result<String, LLMError> {
                unimplemented!()
            }
        }

        #[async_trait::async_trait]
        impl llm::tts::TextToSpeechProvider for AlwaysErrorLlmProvider {}

        #[async_trait::async_trait]
        impl llm::models::ModelsProvider for AlwaysErrorLlmProvider {}

        impl LLMProvider for AlwaysErrorLlmProvider {}

        let candidate: Shop = Faker.fake();
        let repository = MockShopDynamoDbRepository::default();
        let get_shop_service = MockGetShopService::default();

        let mut query_shop_service = MockQueryShopService::default();
        query_shop_service
            .expect_search_shops()
            .return_once(move |_, _, _| {
                Box::pin(async move {
                    Ok(CursoredResult {
                        items: vec![candidate],
                        cursor: Cursor {
                            size: 1,
                            search_after: None,
                        },
                        total: Some(1),
                    })
                })
            });

        let command_shop_service = MockCommandShopService::default();

        let service = SellerServiceImpl {
            repository: &repository,
            get_shop_service: &get_shop_service,
            query_shop_service: &query_shop_service,
            command_shop_service: &command_shop_service,
            llm: Box::new(AlwaysErrorLlmProvider),
        };

        let raw_name: ShopName = Faker.fake();
        let result = service.create_seller_shop_details(&raw_name).await;

        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            SellerServiceError::LLMError(_)
        ));
    }

    #[tokio::test]
    async fn should_propagate_command_error_for_create_seller_shop_details() {
        let repository = MockShopDynamoDbRepository::default();
        let get_shop_service = MockGetShopService::default();

        let mut query_shop_service = MockQueryShopService::default();
        query_shop_service
            .expect_search_shops()
            .return_once(|_, _, _| {
                Box::pin(async {
                    Ok(CursoredResult {
                        items: vec![],
                        cursor: Cursor {
                            size: 0,
                            search_after: None,
                        },
                        total: Some(0),
                    })
                })
            });

        let mut command_shop_service = MockCommandShopService::default();
        command_shop_service
            .expect_create()
            .return_once(|_| Box::pin(async { Err(CommandShopError::SdkBatchGetItemUnprocessed) }));

        let service = SellerServiceImpl {
            repository: &repository,
            get_shop_service: &get_shop_service,
            query_shop_service: &query_shop_service,
            command_shop_service: &command_shop_service,
            llm: Box::new(MockLlmProvider),
        };

        let raw_name: ShopName = Faker.fake();
        let result = service.create_seller_shop_details(&raw_name).await;

        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            SellerServiceError::CommandShopError(_)
        ));
    }

    // ── get_seller_shop_details ──────────────────────────────────────────────

    #[tokio::test]
    async fn should_return_details_when_record_exists_for_get_seller_shop_details() {
        let record: RawShopNameRecord = Faker.fake();
        let expected_shop_id = record.shop_id;
        let expected_slug = record.shop_slug_id.clone();
        let expected_name = record.name.clone();

        let mut repository = MockShopDynamoDbRepository::default();
        repository
            .expect_get_raw_shop_name_record()
            .return_once(move |_| Box::pin(async move { Ok(Some(record)) }));

        let get_shop_service = MockGetShopService::default();
        let query_shop_service = MockQueryShopService::default();
        let command_shop_service = MockCommandShopService::default();

        let service = SellerServiceImpl {
            repository: &repository,
            get_shop_service: &get_shop_service,
            query_shop_service: &query_shop_service,
            command_shop_service: &command_shop_service,
            llm: Box::new(MockLlmProvider),
        };

        let raw_name: ShopName = Faker.fake();
        let result = service.get_seller_shop_details(&raw_name).await.unwrap();

        assert_eq!(expected_shop_id, result.0);
        assert_eq!(expected_slug, result.1);
        assert_eq!(expected_name, result.2);
    }

    #[tokio::test]
    async fn should_create_when_not_found_for_get_seller_shop_details() {
        let new_shop: Shop = Faker.fake();
        let expected_shop_id = new_shop.shop_id;

        let mut repository = MockShopDynamoDbRepository::default();
        repository
            .expect_get_raw_shop_name_record()
            .return_once(|_| Box::pin(async { Ok(None) }));
        repository
            .expect_put_raw_shop_name_record()
            .return_once(|_| Box::pin(async { Ok(PutItemOutput::builder().build()) }));

        let get_shop_service = MockGetShopService::default();

        let mut query_shop_service = MockQueryShopService::default();
        query_shop_service
            .expect_search_shops()
            .return_once(|_, _, _| {
                Box::pin(async {
                    Ok(CursoredResult {
                        items: vec![],
                        cursor: Cursor {
                            size: 0,
                            search_after: None,
                        },
                        total: Some(0),
                    })
                })
            });

        let mut command_shop_service = MockCommandShopService::default();
        command_shop_service
            .expect_create()
            .return_once(move |_| Box::pin(async move { Ok(new_shop) }));

        let service = SellerServiceImpl {
            repository: &repository,
            get_shop_service: &get_shop_service,
            query_shop_service: &query_shop_service,
            command_shop_service: &command_shop_service,
            llm: Box::new(MockLlmProvider),
        };

        let raw_name: ShopName = Faker.fake();
        let result = service.get_seller_shop_details(&raw_name).await.unwrap();

        assert_eq!(expected_shop_id, result.0);
    }

    #[tokio::test]
    async fn should_propagate_error_from_find_for_get_seller_shop_details() {
        let mut repository = MockShopDynamoDbRepository::default();
        repository
            .expect_get_raw_shop_name_record()
            .return_once(|_| {
                Box::pin(async { Err(SdkError::construction_failure("simulated error")) })
            });

        let get_shop_service = MockGetShopService::default();
        let query_shop_service = MockQueryShopService::default();
        let command_shop_service = MockCommandShopService::default();

        let service = SellerServiceImpl {
            repository: &repository,
            get_shop_service: &get_shop_service,
            query_shop_service: &query_shop_service,
            command_shop_service: &command_shop_service,
            llm: Box::new(MockLlmProvider),
        };

        let raw_name: ShopName = Faker.fake();
        let result = service.get_seller_shop_details(&raw_name).await;

        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            SellerServiceError::SdkGetItemError(_)
        ));
    }
}
