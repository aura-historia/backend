use serde::Deserialize;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tracing::info;

#[derive(Debug, Clone, Deserialize)]
pub struct TestMailAppResponse {
    pub emails: Vec<TestMailAppMail>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct TestMailAppMail {
    pub subject: String,
}

pub fn get_test_mail() -> String {
    let test_mail_app_namespace = std::env::var("TEST_MAIL_APP_NAMESPACE").expect("shouldn't fail because env-var 'TEST_MAIL_APP_NAMESPACE' is set as env-var in CI via action-variables");
    format!("{test_mail_app_namespace}.test@inbox.testmail.app")
}

pub async fn wait_for_email(subject_contains: &str) -> bool {
    let test_mail_app_api_key = std::env::var("TEST_MAIL_APP_API_KEY").expect("shouldn't fail because env-var 'TEST_MAIL_APP_API_KEY' is set as env-var in CI via action-secrets");
    let test_mail_app_namespace = std::env::var("TEST_MAIL_APP_NAMESPACE").expect("shouldn't fail because env-var 'TEST_MAIL_APP_NAMESPACE' is set as env-var in CI via action-variables");
    let timestamp_from = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("shouldn't fail because time can't go backwards (pray)")
        .as_millis();
    let res = reqwest::Client::new()
        .get("https://api.testmail.app/api/json")
        .query(&[
            ("apikey", test_mail_app_api_key),
            ("namespace", test_mail_app_namespace),
            ("livequery", "true".to_owned()),
            ("timestamp_from", timestamp_from.to_string()),
        ])
        .timeout(Duration::from_secs(120))
        .send()
        .await;

    match res {
        Ok(response) => {
            if response.status().is_success() {
                response
                    .json::<TestMailAppResponse>()
                    .await
                    .unwrap()
                    .emails
                    .iter()
                    .any(|received_mail| received_mail.subject.contains(subject_contains))
            } else {
                info!(
                    statusCode = response.status().as_u16(),
                    "TestMailApp didn't respond with success (2xx)."
                );
                false
            }
        }
        Err(err) => {
            info!(error = %err, "Failed waiting for email to arrive.");
            false
        }
    }
}
