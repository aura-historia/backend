//! Shared query-wide eligibility capability for public geography and saved matching.
use application::error::BoxError;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct GeographicQueryPermit {
    pub epoch: i64,
    /// Next current public Party-evidence expiry. Its disappearance also fences a query.
    pub eligibility_deadline: Option<time::OffsetDateTime>,
}
#[derive(Debug, thiserror::Error)]
#[error("geographic eligibility authority read failed")]
pub struct GeographicEligibilityError(#[source] pub BoxError);
#[async_trait::async_trait]
pub trait GeographicQueryGate: Send + Sync {
    async fn permit(&self) -> Result<Option<GeographicQueryPermit>, GeographicEligibilityError>;
}
pub async fn with_geographic_eligibility<G, F, Fut, T>(
    gate: &G,
    query: F,
) -> Result<Option<T>, GeographicEligibilityError>
where
    G: GeographicQueryGate + ?Sized,
    F: FnOnce() -> Fut,
    Fut: std::future::Future<Output = Result<T, GeographicEligibilityError>>,
{
    let Some(before) = gate.permit().await? else {
        return Ok(None);
    };
    let result = query().await?;
    if gate.permit().await? != Some(before) {
        return Ok(None);
    }
    Ok(Some(result))
}
