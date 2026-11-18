//! Bootstrap of the Cognito triggers, the only Lambda functions left after the move to the
//! host. They hold no database configuration: each reaches only Cognito and the host's public
//! front door. Long-running processes start through `platform-process` instead.

use lambda_runtime::Context;
use std::{
    env,
    time::{Instant, SystemTime, UNIX_EPOCH},
};
use thiserror::Error;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum LambdaBootstrapConfigError {
    #[error("missing required Lambda configuration {name}")]
    Missing { name: &'static str },
    #[error("invalid Lambda configuration {name}")]
    Invalid { name: &'static str },
}

pub fn required_config_from_env(name: &'static str) -> Result<String, LambdaBootstrapConfigError> {
    match env::var(name) {
        Ok(value) if !value.trim().is_empty() => Ok(value),
        Ok(_) | Err(env::VarError::NotPresent) => Err(LambdaBootstrapConfigError::Missing { name }),
        Err(env::VarError::NotUnicode(_)) => Err(LambdaBootstrapConfigError::Invalid { name }),
    }
}

pub fn log_cold_start(component: &'static str, initialization_started_at: Instant) {
    tracing::info!(
        event = "lambda.initialized",
        component,
        cold_start_duration_ms = initialization_started_at.elapsed().as_millis(),
    );
}

pub fn log_invocation_start(component: &'static str, context: &Context) {
    tracing::debug!(
        event = "lambda.invocation.started",
        component,
        request_id = %context.request_id,
        remaining_budget_ms = remaining_budget_ms(context.deadline, epoch_millis()),
    );
}

fn remaining_budget_ms(deadline_epoch_ms: u64, now_epoch_ms: u64) -> u64 {
    deadline_epoch_ms.saturating_sub(now_epoch_ms)
}

fn epoch_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| u64::try_from(duration.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_clamp_elapsed_invocation_deadline_to_zero() {
        assert_eq!(0, remaining_budget_ms(100, 101));
    }
}
