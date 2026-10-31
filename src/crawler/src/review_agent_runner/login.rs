//! The startup probe of the CLI's login: `claude auth status --json` must report a login with a
//! Claude subscription (`claude.ai`, or an OAuth token of one), so an expired or missing login
//! stops the runner before any review instead of failing every process. The probe logs how the
//! CLI is logged in, never the account's email or a credential.

use super::invocation::ChildEnvironment;
use super::process::{ChildError, ChildOutput, run_cli};
use serde_json::Value;
use std::path::Path;
use std::time::Duration;

const PROBE_TIMEOUT: Duration = Duration::from_secs(60);
/// The login methods of a Claude subscription, as `claude auth status --json` names them.
const SUBSCRIPTION_METHODS: [&str; 2] = ["claude.ai", "oauth_token"];

/// How the CLI is logged in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CliLogin {
    pub method: String,
    pub organization: Option<String>,
    pub subscription: Option<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum LoginError {
    #[error("cannot run the Claude Code CLI's login probe `claude auth status --json`")]
    Probe(#[source] ChildError),
    #[error(
        "the Claude Code CLI's login probe `claude auth status --json` gave no answer the runner can read (exit code {exit_code:?})"
    )]
    Unreadable { exit_code: Option<i32> },
    #[error(
        "the Claude Code CLI is not logged in; sign in again with `claude` and /login as the crawler's member of the Team, with the runner's CLAUDE_CONFIG_DIR"
    )]
    NotLoggedIn,
    #[error(
        "the Claude Code CLI is logged in with {method}, not with a Claude subscription; sign in with `claude` and /login as the crawler's member of the Team, and remove API keys from the runner's environment"
    )]
    NotSubscription { method: String },
}

/// Probes the login of the CLI at `claude`, started with `env` as the agent processes are.
pub async fn probe_login(claude: &Path, env: &ChildEnvironment) -> Result<CliLogin, LoginError> {
    let output = run_cli(claude, &["auth", "status", "--json"], env, PROBE_TIMEOUT)
        .await
        .map_err(LoginError::Probe)?;
    read_login(&output)
}

fn read_login(output: &ChildOutput) -> Result<CliLogin, LoginError> {
    let unreadable = || LoginError::Unreadable {
        exit_code: output.exit_code,
    };
    let status: Value = serde_json::from_slice(&output.stdout).map_err(|_| unreadable())?;
    let logged_in = status
        .get("loggedIn")
        .and_then(Value::as_bool)
        .ok_or_else(unreadable)?;
    if !logged_in {
        return Err(LoginError::NotLoggedIn);
    }
    let method = status
        .get("authMethod")
        .and_then(Value::as_str)
        .unwrap_or("an unknown method")
        .to_owned();
    if !SUBSCRIPTION_METHODS.contains(&method.as_str()) {
        return Err(LoginError::NotSubscription { method });
    }
    let text = |key: &str| status.get(key).and_then(Value::as_str).map(str::to_owned);
    Ok(CliLogin {
        method,
        organization: text("orgName"),
        subscription: text("subscriptionType"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn output(status: Value, exit_code: i32) -> ChildOutput {
        ChildOutput {
            exit_code: Some(exit_code),
            stdout: serde_json::to_vec_pretty(&status).unwrap_or_default(),
            stderr_tail: String::new(),
            stopped: None,
        }
    }

    #[test]
    fn should_accept_a_login_with_a_claude_subscription() {
        let login = read_login(&output(
            json!({
                "loggedIn": true,
                "authMethod": "claude.ai",
                "apiProvider": "firstParty",
                "email": "crawler@example.com",
                "orgName": "Aura Historia",
                "subscriptionType": "team",
            }),
            0,
        ));

        assert_eq!(
            login.ok(),
            Some(CliLogin {
                method: "claude.ai".to_owned(),
                organization: Some("Aura Historia".to_owned()),
                subscription: Some("team".to_owned()),
            })
        );
        assert!(
            read_login(&output(
                json!({ "loggedIn": true, "authMethod": "oauth_token" }),
                0
            ))
            .is_ok()
        );
    }

    #[test]
    fn should_refuse_to_run_without_a_subscription_login() {
        assert!(matches!(
            read_login(&output(
                json!({ "loggedIn": false, "authMethod": "none" }),
                1
            )),
            Err(LoginError::NotLoggedIn)
        ));
        assert!(matches!(
            read_login(&output(
                json!({ "loggedIn": true, "authMethod": "api_key", "apiKeySource": "ANTHROPIC_API_KEY" }),
                0
            )),
            Err(LoginError::NotSubscription { method }) if method == "api_key"
        ));
        let garbled = ChildOutput {
            exit_code: Some(2),
            stdout: b"error: unknown command".to_vec(),
            stderr_tail: String::new(),
            stopped: None,
        };
        assert!(matches!(
            read_login(&garbled),
            Err(LoginError::Unreadable { exit_code: Some(2) })
        ));
        assert!(matches!(
            read_login(&output(json!({ "authMethod": "claude.ai" }), 0)),
            Err(LoginError::Unreadable { .. })
        ));
    }
}
