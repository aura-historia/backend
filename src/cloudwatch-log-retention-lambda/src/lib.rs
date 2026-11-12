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
    let consent_evidence_log_groups = configured_consent_evidence_log_groups()?;
    let existing_retention_days =
        if is_consent_evidence_log_group(&log_group_name, &consent_evidence_log_groups) {
            None
        } else {
            existing_retention_days(client, &log_group_name).await?
        };

    let Some(retention_days) = automatic_retention_days(
        &log_group_name,
        &consent_evidence_log_groups,
        existing_retention_days,
    ) else {
        info!(
            logGroupName = %log_group_name,
            "Skipped automatic retention for a consent evidence log group or one that already has a retention policy."
        );
        return Ok(());
    };

    client
        .put_retention_policy()
        .log_group_name(&log_group_name)
        .retention_in_days(retention_days)
        .send()
        .await?;

    info!(
        logGroupName = %log_group_name,
        retentionDays = retention_days,
        "Set CloudWatch log retention policy."
    );

    Ok(())
}

/// The retention a log group has when the `CreateLogGroup` event arrives, which is already the one
/// its creator declared when CloudFormation created it with a retention.
async fn existing_retention_days(
    client: &Client,
    log_group_name: &str,
) -> Result<Option<i32>, lambda_runtime::Error> {
    let output = client
        .describe_log_groups()
        .log_group_name_prefix(log_group_name)
        .send()
        .await?;
    // Groups sort by name, so a group sorts before every longer name that starts with it.
    Ok(output
        .log_groups()
        .iter()
        .find(|group| group.log_group_name() == Some(log_group_name))
        .and_then(|group| group.retention_in_days()))
}

fn configured_consent_evidence_log_groups() -> Result<Vec<String>, lambda_runtime::Error> {
    let configured = std::env::var("CONSENT_EVIDENCE_LOG_GROUPS")
        .map_err(|_| "missing consent evidence log group exemption configuration")?;
    parse_consent_evidence_log_groups(&configured)
}

fn parse_consent_evidence_log_groups(
    configured: &str,
) -> Result<Vec<String>, lambda_runtime::Error> {
    let names: Vec<String> = serde_json::from_str(configured)?;
    if names.len() != 2
        || names.iter().any(|name| {
            !name.starts_with("/aws/lambda/")
                || name.is_empty()
                || name
                    .chars()
                    .any(|character| matches!(character, '*' | '?' | '[' | ']'))
        })
        || names[0] == names[1]
    {
        return Err("invalid consent evidence log group exemption configuration".into());
    }
    Ok(names)
}

fn is_consent_evidence_log_group(log_group_name: &str, exemptions: &[String]) -> bool {
    exemptions.iter().any(|name| name == log_group_name)
}

/// The default retention for a new log group, unless it holds consent evidence, which never
/// expires automatically, or its creator already chose a retention, which the default must not
/// replace.
fn automatic_retention_days(
    log_group_name: &str,
    exemptions: &[String],
    existing_retention_days: Option<i32>,
) -> Option<i32> {
    (!is_consent_evidence_log_group(log_group_name, exemptions)
        && existing_retention_days.is_none())
    .then_some(LOG_RETENTION_DAYS)
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

    use super::{
        LOG_RETENTION_DAYS, automatic_retention_days, extract_log_group_name,
        is_consent_evidence_log_group, parse_consent_evidence_log_groups,
    };

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

    #[test]
    fn should_skip_only_exact_consent_evidence_log_groups() {
        let exemptions = vec![
            "/aws/lambda/aura-historia-api-prod".to_owned(),
            "/aws/lambda/cognito-post-confirmation-prod".to_owned(),
        ];

        assert!(is_consent_evidence_log_group(
            "/aws/lambda/aura-historia-api-prod",
            &exemptions
        ));
        assert!(is_consent_evidence_log_group(
            "/aws/lambda/cognito-post-confirmation-prod",
            &exemptions
        ));
        assert_eq!(
            automatic_retention_days("/aws/lambda/aura-historia-api-prod", &exemptions, None),
            None
        );
        assert_eq!(
            automatic_retention_days(
                "/aws/lambda/cognito-post-confirmation-prod",
                &exemptions,
                None
            ),
            None
        );
        assert!(!is_consent_evidence_log_group(
            "/aws/lambda/aura-historia-api-dev",
            &exemptions
        ));
        assert!(!is_consent_evidence_log_group(
            "/aws/lambda/other-prod",
            &exemptions
        ));
        assert_eq!(
            automatic_retention_days("/aws/lambda/aura-historia-api-dev", &exemptions, None),
            Some(LOG_RETENTION_DAYS)
        );
        assert_eq!(
            automatic_retention_days("/aws/lambda/other-prod", &exemptions, None),
            Some(LOG_RETENTION_DAYS)
        );
    }

    #[test]
    fn should_keep_the_retention_a_log_group_was_created_with() {
        let exemptions = vec![
            "/aws/lambda/aura-historia-api-prod".to_owned(),
            "/aws/lambda/cognito-post-confirmation-prod".to_owned(),
        ];

        assert_eq!(
            automatic_retention_days("/aws/lambda/other-prod", &exemptions, Some(7)),
            None
        );
        assert_eq!(
            automatic_retention_days("/aws/lambda/other-prod", &exemptions, None),
            Some(LOG_RETENTION_DAYS)
        );
    }

    #[test]
    fn should_validate_stage_exact_exemptions_and_keep_normal_retention_at_30_days() {
        let exemptions = parse_consent_evidence_log_groups(
            r#"["/aws/lambda/aura-historia-api-prod","/aws/lambda/cognito-post-confirmation-prod"]"#,
        )
        .unwrap();

        assert_eq!(exemptions.len(), 2);
        assert!(!is_consent_evidence_log_group(
            "/aws/lambda/other-prod",
            &exemptions
        ));
        assert_eq!(LOG_RETENTION_DAYS, 30);
        assert!(
            parse_consent_evidence_log_groups(
                r#"["/aws/lambda/*","/aws/lambda/cognito-post-confirmation-prod"]"#
            )
            .is_err()
        );
    }
}
