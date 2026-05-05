use fxrate::service::FxRateService;
use lambda_runtime::LambdaEvent;
use tracing::{info, warn};

#[tracing::instrument(skip(service, event), fields(requestId = %event.context.request_id))]
pub async fn handler(
    service: &(dyn FxRateService + Send + Sync),
    event: LambdaEvent<serde_json::Value>,
) -> Result<(), lambda_runtime::Error> {
    let update_res = service.update_current().await;
    match update_res {
        Ok(_) => {
            info!("Updated FxRatesRecord.");
            Ok(())
        }
        Err(err) => {
            warn!(error = %err, "Failed updating FxRatesRecord.");
            Err(err.into())
        }
    }
}
