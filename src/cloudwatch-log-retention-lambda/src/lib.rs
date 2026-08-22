use aws_sdk_cloudwatchlogs::Client;
use lambda_runtime::LambdaEvent;
use serde::Deserialize;
use tracing::{info, warn};

pub const LOG_RETENTION_DAYS: i32 = 30;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreateLogGroupDetail {
    request_parameters: CreateLogGroupRequestParameters,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreateLogGroupRequestParameters {
    log_group_name: String,
}

#[tracing::instrument(skip(client, event), fields(requestId = %event.context.request_id))]
pub async fn handler(
    client: &Client,
    event: LambdaEvent<serde_json::Value>,
) -> Result<(), lambda_runtime::Error> {
    let log_group_name = extract_log_group_name(&event.payload)?;

    client
        .put_retention_policy()
        .log_group_name(&log_group_name)
        .retention_in_days(LOG_RETENTION_DAYS)
        .send()
        .await?;

    info!(
        logGroupName = %log_group_name,
        retentionDays = LOG_RETENTION_DAYS,
        "Set CloudWatch log retention policy."
    );

    Ok(())
}

fn extract_log_group_name(event: &serde_json::Value) -> Result<String, lambda_runtime::Error> {
    let detail = event
        .get("detail")
        .cloned()
        .ok_or("missing EventBridge event detail")?;
    let detail: CreateLogGroupDetail = serde_json::from_value(detail)?;

    if detail.request_parameters.log_group_name.trim().is_empty() {
        warn!("Received CreateLogGroup event without a log group name.");
        return Err("missing logGroupName in CreateLogGroup event".into());
    }

    Ok(detail.request_parameters.log_group_name)
}

#[cfg(test)]
mod tests {
    use rstest::rstest;
    use serde_json::json;

    use super::extract_log_group_name;

    #[rstest]
    #[case("/aws/lambda/shopify-lambda-prod")]
    #[case("custom/application/log-group")]
    fn should_extract_log_group_name_when_create_log_group_event_for_retention(
        #[case] expected_log_group_name: &str,
    ) {
        let event = json!({
            "version": "0",
            "id": "1234",
            "detail-type": "AWS API Call via CloudTrail",
            "source": "aws.logs",
            "account": "123456789012",
            "time": "2026-06-14T12:00:00Z",
            "region": "eu-central-1",
            "resources": [],
            "detail": {
                "eventSource": "logs.amazonaws.com",
                "eventName": "CreateLogGroup",
                "requestParameters": {
                    "logGroupName": expected_log_group_name
                }
            }
        });

        let log_group_name = extract_log_group_name(&event).unwrap();

        assert_eq!(log_group_name, expected_log_group_name);
    }

    #[test]
    fn should_return_error_when_detail_is_missing_for_retention() {
        let event = json!({});

        let err = extract_log_group_name(&event).unwrap_err();

        assert_eq!(err.to_string(), "missing EventBridge event detail");
    }

    #[test]
    fn should_return_error_when_log_group_name_is_missing_for_retention() {
        let event = json!({
            "detail": {
                "requestParameters": {}
            }
        });

        let err = extract_log_group_name(&event).unwrap_err();

        assert!(err.to_string().contains("missing field `logGroupName`"));
    }

    #[test]
    fn should_return_error_when_log_group_name_is_blank_for_retention() {
        let event = json!({
            "detail": {
                "requestParameters": {
                    "logGroupName": "   "
                }
            }
        });

        let err = extract_log_group_name(&event).unwrap_err();

        assert_eq!(
            err.to_string(),
            "missing logGroupName in CreateLogGroup event"
        );
    }
}
