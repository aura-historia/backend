use lambda_runtime::{Error, LambdaEvent, run, service_fn};
use large_language_model::{
    AnthropicClaude, AnthropicConfig, ClaudeHaiku55Thinking, ClaudeModel,
    ClaudeReducedThinkingEffort,
};
use platform_lambda_bootstrap::{
    LazyComposition, RuntimePostgresConfig, log_cold_start, log_invocation_start,
    required_config_from_env,
};
use platform_lambda_sqs::handle_sqs_invocation;
use platform_observability::{LoggingConfig, init};
use product_listing_service::use_cases::TranslateProductListingEventUseCase;
use product_translation_lambda::{
    compose_product_translation_use_case, handler_with_invocation_budget, invocation_budget,
};
use std::{future::Future, sync::Arc, time::Instant};

const COMPONENT: &str = "product-translation-lambda";
const ANTHROPIC_API_KEY_ENV: &str = "ANTHROPIC_API_KEY";
// Title translation needs no reasoning, so thinking stays off and the translator's output-token
// limit remains reserved for the structured answer.
const TRANSLATION_MODEL: ClaudeModel = ClaudeModel::Haiku55(ClaudeHaiku55Thinking::Disabled(
    ClaudeReducedThinkingEffort::Low,
));

fn main() -> Result<(), Error> {
    let initialization_started_at = Instant::now();
    init(LoggingConfig::from_env());
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|_| Error::from("failed to build Lambda async runtime"))?
        .block_on(async_main(initialization_started_at))
}

async fn async_main(initialization_started_at: Instant) -> Result<(), Error> {
    let postgres = RuntimePostgresConfig::from_env()?;
    let anthropic = AnthropicCredentials::from_env()?;
    let use_cases = Arc::new(LazyComposition::new());

    log_cold_start(COMPONENT, initialization_started_at);
    run(service_fn(
        move |event: LambdaEvent<aws_lambda_events::sqs::SqsEvent>| {
            let postgres = postgres.clone();
            let anthropic = anthropic.clone();
            let use_cases = Arc::clone(&use_cases);
            async move {
                log_invocation_start(COMPONENT, &event.context);
                handle_invocation(event, move || async move {
                    use_cases
                        .get_or_try_build(|| async {
                            let pool = postgres.connect(COMPONENT).await?;
                            let model = anthropic.large_language_model()?;
                            Ok::<_, Error>(compose_product_translation_use_case(pool, model))
                        })
                        .await
                })
                .await
            }
        },
    ))
    .await
}

type ProductTranslationUseCase = Arc<dyn TranslateProductListingEventUseCase>;

async fn handle_invocation<F, Fut>(
    event: LambdaEvent<aws_lambda_events::sqs::SqsEvent>,
    setup: F,
) -> Result<aws_lambda_events::sqs::SqsBatchResponse, Error>
where
    F: FnOnce() -> Fut,
    Fut: Future<Output = Result<ProductTranslationUseCase, Error>>,
{
    handle_sqs_invocation(
        event,
        COMPONENT,
        invocation_budget,
        setup,
        |event, use_case, budget| async move {
            handler_with_invocation_budget(event, use_case.as_ref(), &budget).await
        },
    )
    .await
}

#[derive(Clone)]
struct AnthropicCredentials {
    api_key: String,
}

impl AnthropicCredentials {
    fn from_env() -> Result<Self, Error> {
        Ok(Self {
            api_key: required_config_from_env(ANTHROPIC_API_KEY_ENV)?,
        })
    }

    fn large_language_model(&self) -> Result<AnthropicClaude, Error> {
        AnthropicClaude::new(AnthropicConfig::new(TRANSLATION_MODEL), &self.api_key)
            .map_err(|_| Error::from("failed to configure Anthropic client"))
    }
}
