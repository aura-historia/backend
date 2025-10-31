use lambda_runtime::LambdaEvent;

#[tracing::instrument(skip(client, event), fields(requestId = %event.context.request_id))]
pub async fn handler(
    client: &aws_sdk_autoscaling::Client,
    asg_name: &str,
    event: LambdaEvent<serde_json::Value>,
) -> Result<(), lambda_runtime::Error> {
    let _ = client
        .update_auto_scaling_group()
        .auto_scaling_group_name(asg_name)
        .desired_capacity(0)
        .send()
        .await?;

    Ok(())
}
