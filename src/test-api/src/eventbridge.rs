use crate::localstack::get_aws_config;
use tokio::sync::OnceCell;

static EVENTBRIDGE_CLIENT: OnceCell<aws_sdk_eventbridge::Client> = OnceCell::const_new();

pub async fn get_eventbridge_client() -> &'static aws_sdk_eventbridge::Client {
    EVENTBRIDGE_CLIENT
        .get_or_init(|| async { aws_sdk_eventbridge::Client::new(get_aws_config().await) })
        .await
}
