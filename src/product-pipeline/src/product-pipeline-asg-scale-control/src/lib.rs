use aws_sdk_cloudwatch::types::{Datapoint, Dimension};
use lambda_runtime::LambdaEvent;
use std::time::SystemTime;
use time::{OffsetDateTime, ext::NumericalDuration};
use tracing::{error, info, warn};

#[derive(Debug, Clone)]
pub struct SqsAsgComponent {
    pub sqs_url: String,
    pub queue_name: String,
    pub asg_name: String,
}

#[tracing::instrument(skip(autoscaling, sqs, cloudwatch, components, event), fields(requestId = %event.context.request_id))]
pub async fn handler(
    autoscaling: &aws_sdk_autoscaling::Client,
    sqs: &aws_sdk_sqs::Client,
    cloudwatch: &aws_sdk_cloudwatch::Client,
    components: &[SqsAsgComponent],
    event: LambdaEvent<serde_json::Value>,
) -> Result<(), lambda_runtime::Error> {
    for component in components {
        let handle_res = handle_sqs_asg_component(autoscaling, sqs, cloudwatch, component).await;
        if let Err(err) = handle_res {
            error!(
                error = ?err,
                queue = component.queue_name,
                asg = component.asg_name,
                "Failed handling SQS-ASG-Component.",
            );
        }
    }
    Ok(())
}

async fn handle_sqs_asg_component(
    autoscaling: &aws_sdk_autoscaling::Client,
    sqs: &aws_sdk_sqs::Client,
    cloudwatch: &aws_sdk_cloudwatch::Client,
    component: &SqsAsgComponent,
) -> Result<(), lambda_runtime::Error> {
    let queue_attributes = sqs
        .get_queue_attributes()
        .queue_url(&component.sqs_url)
        .attribute_names(aws_sdk_sqs::types::QueueAttributeName::All)
        .send()
        .await?
        .attributes
        .unwrap_or_default();
    let queue_visible_messages = queue_attributes
        .get(&aws_sdk_sqs::types::QueueAttributeName::ApproximateNumberOfMessages)
        .ok_or("Get-Queue-Attributes response did not contain 'ApproximateNumberOfMessages'.")?
        .parse::<u32>()?;

    // CloudWatch-Metrics are eventual consistent
    let end_time = OffsetDateTime::now_utc()
        .checked_sub(5.seconds())
        .expect("shouldn't fail subtracting 5 seconds from now");
    let start_time = end_time
        .checked_sub(30.seconds())
        .expect("shouldn't fail subtracting 30 seconds from now-5sec");
    let queue_oldest_message_response = cloudwatch
        .get_metric_statistics()
        .namespace("AWS/SQS")
        .metric_name("ApproximateAgeOfOldestMessage")
        .dimensions(
            Dimension::builder()
                .name("QueueName")
                .value(&component.queue_name)
                .build(),
        )
        .start_time(SystemTime::from(start_time).into())
        .end_time(SystemTime::from(end_time).into())
        .period(60)
        .statistics(aws_sdk_cloudwatch::types::Statistic::Maximum)
        .send()
        .await?;

    let queue_oldest_message = queue_oldest_message_response
        .datapoints
        .unwrap_or_default()
        .into_iter()
        .max_by_key(|datapoint| datapoint.timestamp().cloned())
        .unwrap_or_else(|| {
            warn!("CloudWatch-GetMetricStatistics response did not contain any data-points for 'ApproximateAgeOfOldestMessage'. Defaulting to an empty datapoint.");
            Datapoint::builder().build()
        })
        .maximum()
        .unwrap_or_else(|| {
            warn!("CloudWatch-GetMetricStatistics response did not contain 'maximum' 'ApproximateAgeOfOldestMessage'. Defaulting to '0'.");
            0f64
        }) as u32;

    let current_desired_asg_capacity = autoscaling
        .describe_auto_scaling_groups()
        .auto_scaling_group_names(&component.asg_name)
        .send()
        .await?
        .auto_scaling_groups
        .unwrap_or_default()
        .first()
        .ok_or("Describe-ASG response did not contain any asg.")?
        .desired_capacity()
        .ok_or("Describe-ASG response did not contain 'desired_capacity' for first asg.")?
        as u16;
    let new_desired_asg_capacity = compute_desired_capacity(
        current_desired_asg_capacity,
        queue_visible_messages,
        queue_oldest_message,
    );

    if new_desired_asg_capacity != current_desired_asg_capacity {
        let _ = autoscaling
            .update_auto_scaling_group()
            .auto_scaling_group_name(&component.asg_name)
            .desired_capacity(new_desired_asg_capacity as i32)
            .send()
            .await?;
        info!(
            asg = component.asg_name,
            oldDesiredCapacity = current_desired_asg_capacity,
            newDesiredCapacity = new_desired_asg_capacity,
            sqsVisibleMessages = queue_visible_messages,
            sqsOldestMessage = queue_oldest_message,
            "Updated desired capacity for auto-scaling-group."
        );
    } else {
        info!(
            asg = component.asg_name,
            desiredCapacity = current_desired_asg_capacity,
            sqsVisibleMessages = queue_visible_messages,
            sqsOldestMessage = queue_oldest_message,
            "Keeping current desired capacity for auto-scaling-group."
        );
    }

    Ok(())
}

fn compute_desired_capacity(
    current_desired_asg_capacity: u16,
    queue_visible_messages: u32,
    queue_oldest_message: u32,
) -> u16 {
    if current_desired_asg_capacity == 0 {
        if queue_visible_messages < 10_000 {
            if queue_oldest_message < 86_400 { 0 } else { 1 }
        } else {
            (queue_visible_messages as f64 / 50_000f64).floor() as u16
        }
    } else {
        current_desired_asg_capacity
    }
}

#[cfg(test)]
mod tests {
    use crate::compute_desired_capacity;

    #[rstest::rstest]
    #[case(0, 0, 0, 0)]
    #[case(0, 42, 300, 0)]
    #[case(0, 42, 99_999, 1)]
    #[case(0, 420, 99_999, 1)]
    #[case(0, 5412, 99_999, 1)]
    #[case(0, 54120, 1, 1)]
    #[case(0, 77_891, 100, 1)]
    #[case(0, 77_891, 99_999, 1)]
    #[case(0, 99_999, 37, 1)]
    #[case(0, 100_001, 37, 2)]
    #[case(0, 199_999, 87455, 3)]
    #[case(1, 42, 300, 1)]
    #[case(2, 42, 99_999, 2)]
    #[case(3, 420, 99_999, 3)]
    #[case(4, 5412, 99_999, 4)]
    #[case(5, 54120, 1, 5)]
    #[case(6, 77_891, 100, 6)]
    #[case(7, 77_891, 99_999, 7)]
    #[case(8, 99_999, 37, 8)]
    #[case(9, 100_001, 37, 9)]
    #[case(10, 199_999, 87455, 10)]
    #[trace]
    fn should_compute_desired_capacity(
        #[case] current_desired_asg_capacity: u16,
        #[case] queue_visible_messages: u32,
        #[case] queue_oldest_message: u32,
        #[case] expected: u16,
    ) {
        let actual = compute_desired_capacity(
            current_desired_asg_capacity,
            queue_visible_messages,
            queue_oldest_message,
        );
        assert_eq!(expected, actual);
    }
}
