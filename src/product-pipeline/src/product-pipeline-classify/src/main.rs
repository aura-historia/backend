use aws_config::BehaviorVersion;
use aws_lambda_events::sqs::SqsEvent;
use aws_sdk_dynamodb::Client;
use lambda_runtime::{Error, LambdaEvent, run, service_fn};
use product::dynamodb::repository::ProductDynamoDbRepositoryImpl;
use product::service::get_service::GetProductServiceImpl;
use product_classification::{
    category::{
        dynamodb_repository::CategoryDynamoDbRepositoryImpl,
        opensearch_repository::CategoryOpenSearchRepositoryImpl, service::CategoryServiceImpl,
    },
    period::{
        dynamodb_repository::PeriodDynamoDbRepositoryImpl,
        opensearch_repository::PeriodOpenSearchRepositoryImpl, service::PeriodServiceImpl,
    },
};
use product_pipeline_classify::{handler, service::ClassificationServiceImpl};
use tracing::debug;

#[tokio::main]
async fn main() -> Result<(), Error> {
    common::logging::init_logging();

    let aws_config = aws_config::defaults(BehaviorVersion::v2026_01_12())
        .load()
        .await;

    let table_name = std::env::var("DYNAMODB_TABLE_NAME")
        .expect("shouldn't fail loading env-var 'DYNAMODB_TABLE_NAME'");

    let client = Client::new(&aws_config);
    let product_repository = ProductDynamoDbRepositoryImpl::new(&client, &table_name);
    let get_product_service = GetProductServiceImpl::new(&product_repository);

    let opensearch = common::opensearch::client::load_client()
        .await
        .expect("shouldn't fail loading OpenSearch-Client");
    let category_dynamodb_repository = CategoryDynamoDbRepositoryImpl::new(&client, &table_name);
    let category_opensearch_repository = CategoryOpenSearchRepositoryImpl::new(&opensearch);
    let category_service = CategoryServiceImpl::new(
        &category_dynamodb_repository,
        &category_opensearch_repository,
    );
    let period_dynamodb_repository = PeriodDynamoDbRepositoryImpl::new(&client, &table_name);
    let period_opensearch_repository = PeriodOpenSearchRepositoryImpl::new(&opensearch);
    let period_service =
        PeriodServiceImpl::new(&period_dynamodb_repository, &period_opensearch_repository);

    debug!("Lambda initialized.");

    if std::env::var("LOCALSTACK_HOSTNAME").is_ok() {
        use product_pipeline_classify::service::MockClassificationService;

        let mut classification_service = MockClassificationService::new();
        classification_service
            .expect_classify()
            .returning(|_, categories, periods| {
                let cat = categories[0].clone();
                let per = periods[0].clone();
                Box::pin(async move { Ok((cat, per)) })
            });
        run(service_fn(|event: LambdaEvent<SqsEvent>| async {
            handler(
                &classification_service,
                &get_product_service,
                &product_repository,
                &category_service,
                &period_service,
                event,
            )
            .await
        }))
        .await
    } else {
        let gemini_api_key = std::env::var("GEMINI_API_KEY")
            .expect("shouldn't fail loading env-var 'GEMINI_API_KEY'");
        let classification_service = ClassificationServiceImpl::new(&gemini_api_key);
        run(service_fn(|event: LambdaEvent<SqsEvent>| async {
            handler(
                &classification_service,
                &get_product_service,
                &product_repository,
                &category_service,
                &period_service,
                event,
            )
            .await
        }))
        .await
    }
}
