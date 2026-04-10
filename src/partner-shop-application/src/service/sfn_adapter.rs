use async_trait::async_trait;

#[derive(Debug, thiserror::Error)]
pub enum SfnAdapterError {
    #[error("Failed to start step function execution: {0}")]
    StartExecutionError(String),

    #[error("Failed to send task success: {0}")]
    SendTaskSuccessError(String),
}

#[async_trait]
#[mockall::automock]
pub trait SfnAdapter {
    /// Starts a Step Function execution with the given state machine ARN and input JSON.
    async fn start_execution(
        &self,
        state_machine_arn: &str,
        input: &str,
    ) -> Result<String, SfnAdapterError>;

    /// Sends a task success callback to the Step Function with the given task token and output.
    async fn send_task_success(
        &self,
        task_token: &str,
        output: &str,
    ) -> Result<(), SfnAdapterError>;
}

pub struct SfnAdapterImpl<'a> {
    client: &'a aws_sdk_sfn::Client,
}

impl<'a> SfnAdapterImpl<'a> {
    pub fn new(client: &'a aws_sdk_sfn::Client) -> Self {
        Self { client }
    }
}

#[async_trait]
impl<'a> SfnAdapter for SfnAdapterImpl<'a> {
    async fn start_execution(
        &self,
        state_machine_arn: &str,
        input: &str,
    ) -> Result<String, SfnAdapterError> {
        let output = self
            .client
            .start_execution()
            .state_machine_arn(state_machine_arn)
            .input(input)
            .send()
            .await
            .map_err(|e| SfnAdapterError::StartExecutionError(e.to_string()))?;

        Ok(output.execution_arn)
    }

    async fn send_task_success(
        &self,
        task_token: &str,
        output: &str,
    ) -> Result<(), SfnAdapterError> {
        self.client
            .send_task_success()
            .task_token(task_token)
            .output(output)
            .send()
            .await
            .map_err(|e| SfnAdapterError::SendTaskSuccessError(e.to_string()))?;

        Ok(())
    }
}
