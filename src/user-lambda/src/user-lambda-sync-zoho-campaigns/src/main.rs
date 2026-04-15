use aws_config::BehaviorVersion;
use aws_lambda_events::sqs::SqsEvent;
use lambda_runtime::{Error, LambdaEvent, run, service_fn};
use tracing::debug;
use user::service::zoho_campaigns_service::zoho_impl::ZohoCampaignsServiceImpl;
use user_lambda_sync_zoho_campaigns::handler;

#[tokio::main]
async fn main() -> Result<(), Error> {
    common::logging::init_logging();

    let _aws_config = aws_config::defaults(BehaviorVersion::v2026_01_12())
        .load()
        .await;

    let zoho_list_key =
        std::env::var("ZOHO_LIST_KEY").expect("shouldn't fail loading env-var 'ZOHO_LIST_KEY'");
    let zoho_client_id =
        std::env::var("ZOHO_CLIENT_ID").expect("shouldn't fail loading env-var 'ZOHO_CLIENT_ID'");
    let zoho_client_secret = std::env::var("ZOHO_CLIENT_SECRET")
        .expect("shouldn't fail loading env-var 'ZOHO_CLIENT_SECRET'");
    let zoho_refresh_token = std::env::var("ZOHO_REFRESH_TOKEN")
        .expect("shouldn't fail loading env-var 'ZOHO_REFRESH_TOKEN'");
    let zoho_accounts_url = std::env::var("ZOHO_ACCOUNTS_URL")
        .unwrap_or_else(|_| "https://accounts.zoho.eu".to_string());
    let zoho_campaigns_url = std::env::var("ZOHO_CAMPAIGNS_URL")
        .unwrap_or_else(|_| "https://campaigns.zoho.eu".to_string());

    let client = reqwest::Client::new();
    let zoho_service = ZohoCampaignsServiceImpl::new(
        zoho_list_key,
        client,
        zoho_client_id,
        zoho_client_secret,
        zoho_refresh_token,
        zoho_accounts_url,
        zoho_campaigns_url,
    );

    debug!("Lambda initialized.");

    run(service_fn(|event: LambdaEvent<SqsEvent>| async {
        handler(&zoho_service, event).await
    }))
    .await
}
