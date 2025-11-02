use lambda_runtime::LambdaEvent;
use opensearch::http::response::Response;
use serde_json::json;
use tracing::info;

#[tracing::instrument(skip(autoscaling_client, opensearch_client, event), fields(requestId = %event.context.request_id))]
pub async fn handler(
    autoscaling_client: &aws_sdk_autoscaling::Client,
    opensearch_client: &opensearch::OpenSearch,
    asg_name: &str,
    event: LambdaEvent<serde_json::Value>,
) -> Result<(), lambda_runtime::Error> {
    scale_down(autoscaling_client, opensearch_client, asg_name).await
}

pub async fn scale_down(
    autoscaling_client: &aws_sdk_autoscaling::Client,
    opensearch_client: &opensearch::OpenSearch,
    asg_name: &str,
) -> Result<(), lambda_runtime::Error> {
    let _ = autoscaling_client
        .update_auto_scaling_group()
        .auto_scaling_group_name(asg_name)
        .desired_capacity(0)
        .send()
        .await?;
    info!(
        name = asg_name,
        desiredCapacity = 0,
        "Updated auto-scaling-group."
    );

    let _ = opensearch_client
        .indices()
        .put_settings(opensearch::indices::IndicesPutSettingsParts::Index(&[
            "items",
        ]))
        .body(json!({
            "index": {
                "refresh_interval": "5m"
            }
        }))
        .send()
        .await
        .map(Response::error_for_status_code)??;
    info!(refreshInterval = "5m", "Updated refresh-interval.");

    Ok(())
}
