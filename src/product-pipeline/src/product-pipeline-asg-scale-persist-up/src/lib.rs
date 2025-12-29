use lambda_runtime::LambdaEvent;
use tracing::info;

#[tracing::instrument(skip(autoscaling, event), fields(requestId = %event.context.request_id))]
pub async fn handler(
    autoscaling: &aws_sdk_autoscaling::Client,
    asg_name: &str,
    event: LambdaEvent<serde_json::Value>,
) -> Result<(), lambda_runtime::Error> {
    let _ = autoscaling
        .update_auto_scaling_group()
        .auto_scaling_group_name(asg_name)
        .desired_capacity(1)
        .send()
        .await?;
    info!(
        name = asg_name,
        desiredCapacity = 1,
        "Updated auto-scaling-group."
    );

    Ok(())
}
