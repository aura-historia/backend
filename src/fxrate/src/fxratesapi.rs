use common::currency::{data::CurrencyData, domain::Currency};
use serde::Deserialize;
use std::collections::HashMap;
use strum::IntoEnumIterator;

#[derive(Debug, Clone, Deserialize)]
pub struct FxRatesApiResponse {
    pub success: bool,
    pub base: CurrencyData,
    pub rates: HashMap<CurrencyData, f32>,
}

#[async_trait::async_trait]
#[mockall::automock]
pub trait FxRatesApiClient {
    async fn get_fx_rates(&self, base: &Currency) -> Result<FxRatesApiResponse, reqwest::Error>;
}

pub struct FxRatesApiClientImpl<'a> {
    reqwest: &'a reqwest::Client,
    fxrates_api_token: &'a str,
}

impl<'a> FxRatesApiClientImpl<'a> {
    pub fn new(reqwest: &'a reqwest::Client, fxrates_api_token: &'a str) -> Self {
        Self {
            reqwest,
            fxrates_api_token,
        }
    }
}

#[async_trait::async_trait]
impl<'a> FxRatesApiClient for FxRatesApiClientImpl<'a> {
    async fn get_fx_rates(&self, base: &Currency) -> Result<FxRatesApiResponse, reqwest::Error> {
        #[allow(unstable_name_collisions)]
        let currencies: Vec<&str> = Currency::iter()
            .filter(|c| c != base)
            .map(|c| c.as_str())
            .collect();
        self.reqwest
            .get("https://api.fxratesapi.com/latest")
            .query(&[
                ("base", base.as_str()),
                ("currencies", &currencies.join(",")),
            ])
            .bearer_auth(self.fxrates_api_token)
            .send()
            .await?
            .json::<FxRatesApiResponse>()
            .await
    }
}

#[cfg(any())]
#[cfg(test)]
mod tests {
    use crate::fxratesapi::{FxRatesApiClient, FxRatesApiClientImpl};
    use common::currency::domain::Currency;

    #[tokio::test]
    async fn should_get_fx_rates() {
        let reqwest = reqwest::Client::new();
        let client = FxRatesApiClientImpl::new(&reqwest, "foo");

        let res = client.get_fx_rates(&Currency::Eur).await.unwrap();
        println!("{res:?}");
    }
}
