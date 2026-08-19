use common::error::boxed::BoxError;
use money::Currency;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FxRateQuote {
    pub currency: Currency,
    pub units_per_eur: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FxRateQuoteSet {
    pub base: Currency,
    pub quotes: Vec<FxRateQuote>,
}

#[derive(Debug, thiserror::Error)]
pub enum FxRateQuoteProviderError {
    #[error("FX rate provider request failed")]
    RequestFailed {
        #[source]
        source: BoxError,
    },
    #[error("FX rate provider returned an invalid response")]
    InvalidResponse {
        #[source]
        source: BoxError,
    },
}

#[async_trait::async_trait]
pub trait FxRateQuoteProvider: Send + Sync {
    async fn fetch_eur_quotes(&self) -> Result<FxRateQuoteSet, FxRateQuoteProviderError>;
}
