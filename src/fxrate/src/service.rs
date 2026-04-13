use crate::{
    dynamodb::{
        record::{FxRatesRecord, mk_pk, mk_sk, rate_key},
        repository::FxRateDynamoDbRepository,
    },
    fxratesapi::FxRatesApiClient,
};
use aws_sdk_dynamodb::{
    error::SdkError,
    operation::{get_item::GetItemError, put_item::PutItemError},
};
use common::{
    currency::domain::Currency,
    price::domain::{FX_RATE_SCALE, Rate},
};
use std::collections::HashMap;
use strum::{EnumCount, IntoEnumIterator};
use time::OffsetDateTime;

#[derive(Debug, thiserror::Error)]
pub enum FxRateServiceError {
    #[error("ReqwestError: {0:?}")]
    ReqwestError(#[from] reqwest::Error),

    #[error("SdkErrorGetItem: {0:?}")]
    SdkErrorGetItem(#[from] SdkError<GetItemError>),

    #[error("SdkErrorPutItem: {0:?}")]
    SdkErrorPutItem(#[from] SdkError<PutItemError>),

    #[error("FxRatesNotExists: The FxRatesRecord does not exist")]
    FxRatesNotExists,

    #[error("FxratesApiError: The response contained success=false")]
    FxratesApiError,

    #[error("MissingFxRate: Missing FxRate to exchange from '{0}' to '{1}'")]
    MissingFxRate(Currency, Currency),
}

#[async_trait::async_trait]
#[mockall::automock]
pub trait FxRateService {
    async fn update_current(&self) -> Result<FxRatesRecord, FxRateServiceError>;
    async fn get_current(&self) -> Result<FxRatesRecord, FxRateServiceError>;
}

pub struct FxRateServiceImpl<'a> {
    fxrates_api: &'a (dyn FxRatesApiClient + Send + Sync),
    repository: &'a (dyn FxRateDynamoDbRepository + Send + Sync),
}

impl<'a> FxRateServiceImpl<'a> {
    pub fn new(
        fxrates_api: &'a (dyn FxRatesApiClient + Send + Sync),
        repository: &'a (dyn FxRateDynamoDbRepository + Send + Sync),
    ) -> Self {
        Self {
            fxrates_api,
            repository,
        }
    }
}

#[async_trait::async_trait]
impl<'a> FxRateService for FxRateServiceImpl<'a> {
    async fn update_current(&self) -> Result<FxRatesRecord, FxRateServiceError> {
        let mut rates: HashMap<(Currency, Currency), Rate> =
            HashMap::with_capacity(Currency::COUNT.pow(2));
        for src in Currency::iter() {
            let res = self.fxrates_api.get_fx_rates(&src).await?;
            if !res.success {
                return Err(FxRateServiceError::FxratesApiError);
            }
            for (tgt, rate_f32) in res.rates {
                let rate_u64_scaled = (rate_f32 * FX_RATE_SCALE as f32).ceil() as u64;
                rates.insert((src, tgt.into()), rate_u64_scaled);
            }
        }

        let mut rate_map = HashMap::new();
        for src in Currency::iter() {
            for tgt in Currency::iter() {
                if src != tgt {
                    let key = rate_key(&src, &tgt);
                    let rate = rates
                        .get(&(src, tgt))
                        .ok_or(FxRateServiceError::MissingFxRate(src, tgt))?;
                    rate_map.insert(key, *rate);
                }
            }
        }

        let fx_rates_record = FxRatesRecord {
            pk: mk_pk().to_owned(),
            sk: mk_sk().to_owned(),
            rates: rate_map,
            timestamp: OffsetDateTime::now_utc(),
        };

        let _ = self
            .repository
            .put_fx_rates_record(fx_rates_record.clone())
            .await?;

        Ok(fx_rates_record)
    }

    async fn get_current(&self) -> Result<FxRatesRecord, FxRateServiceError> {
        self.repository
            .get_fx_rates_record()
            .await?
            .ok_or(FxRateServiceError::FxRatesNotExists)
    }
}

#[cfg(test)]
mod tests {
    use crate::{
        dynamodb::{record::rate_key, repository::MockFxRateDynamoDbRepository},
        fxratesapi::{FxRatesApiResponse, MockFxRatesApiClient},
        service::{FxRateService, FxRateServiceError, FxRateServiceImpl},
    };
    use aws_sdk_dynamodb::{error::SdkError, operation::put_item::PutItemOutput};
    use common::{currency::data::CurrencyData, currency::domain::Currency};
    use std::collections::HashMap;
    use strum::{EnumCount, IntoEnumIterator};

    fn create_complete_rates_for_base(base: Currency) -> HashMap<CurrencyData, f32> {
        let mut rates = HashMap::new();
        for (i, currency) in Currency::iter().enumerate() {
            if currency != base {
                let base_idx = Currency::iter()
                    .position(|c| c == base)
                    .expect("base currency must be a valid Currency variant");
                let rate = 1.0 + (base_idx as f32 * 0.1) + (i as f32 * 0.05);
                rates.insert(currency.into(), rate);
            }
        }
        rates
    }

    #[tokio::test]
    async fn should_update_current_successfully_when_api_returns_complete_rates() {
        let mut fxrates_api = MockFxRatesApiClient::default();
        fxrates_api
            .expect_get_fx_rates()
            .times(Currency::COUNT)
            .returning(|base| {
                let base = *base;
                Box::pin(async move {
                    Ok(FxRatesApiResponse {
                        success: true,
                        base: base.into(),
                        rates: create_complete_rates_for_base(base),
                    })
                })
            });

        let mut repository = MockFxRateDynamoDbRepository::default();
        repository
            .expect_put_fx_rates_record()
            .once()
            .returning(|_| Box::pin(async { Ok(PutItemOutput::builder().build()) }));

        let service = FxRateServiceImpl::new(&fxrates_api, &repository);
        let result = service.update_current().await;

        assert!(result.is_ok());
        let record = result.unwrap();
        assert_eq!(record.pk, "global#fx_rate");
        assert_eq!(record.sk, "fx_rate#details");

        let expected_count = Currency::COUNT * (Currency::COUNT - 1);
        assert_eq!(record.rates.len(), expected_count);
        for src in Currency::iter() {
            for tgt in Currency::iter() {
                if src != tgt {
                    let key = rate_key(&src, &tgt);
                    assert!(record.rates.contains_key(&key), "missing rate key: {key}");
                    assert!(*record.rates.get(&key).unwrap() > 0);
                }
            }
        }
    }

    #[tokio::test]
    async fn should_err_when_fxrates_api_not_successful() {
        let mut fxrates_api = MockFxRatesApiClient::default();
        fxrates_api.expect_get_fx_rates().return_once(|base| {
            let base = *base;
            Box::pin(async move {
                Ok(FxRatesApiResponse {
                    success: false,
                    base: base.into(),
                    rates: HashMap::default(),
                })
            })
        });
        let mut repository = MockFxRateDynamoDbRepository::default();
        repository.expect_put_fx_rates_record().never();

        let service = FxRateServiceImpl::new(&fxrates_api, &repository);
        let actual = service.update_current().await;

        assert!(actual.is_err());
        assert!(matches!(
            actual.unwrap_err(),
            FxRateServiceError::FxratesApiError
        ));
    }

    #[tokio::test]
    async fn should_err_when_missing_required_rate_for_eur_gbp() {
        let mut fxrates_api = MockFxRatesApiClient::default();
        fxrates_api.expect_get_fx_rates().returning(|base| {
            let base = *base;
            Box::pin(async move {
                let mut rates = create_complete_rates_for_base(base);
                if base == Currency::Eur {
                    rates.remove(&CurrencyData::Gbp);
                }
                Ok(FxRatesApiResponse {
                    success: true,
                    base: base.into(),
                    rates,
                })
            })
        });

        let mut repository = MockFxRateDynamoDbRepository::default();
        repository.expect_put_fx_rates_record().never();

        let service = FxRateServiceImpl::new(&fxrates_api, &repository);
        let result = service.update_current().await;

        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            FxRateServiceError::MissingFxRate(Currency::Eur, Currency::Gbp)
        ));
    }

    #[tokio::test]
    async fn should_err_when_missing_required_rate_for_usd_aud() {
        let mut fxrates_api = MockFxRatesApiClient::default();
        fxrates_api.expect_get_fx_rates().returning(|base| {
            let base = *base;
            Box::pin(async move {
                let mut rates = create_complete_rates_for_base(base);
                if base == Currency::Usd {
                    rates.remove(&CurrencyData::Aud);
                }
                Ok(FxRatesApiResponse {
                    success: true,
                    base: base.into(),
                    rates,
                })
            })
        });

        let mut repository = MockFxRateDynamoDbRepository::default();
        repository.expect_put_fx_rates_record().never();

        let service = FxRateServiceImpl::new(&fxrates_api, &repository);
        let result = service.update_current().await;

        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            FxRateServiceError::MissingFxRate(Currency::Usd, Currency::Aud)
        ));
    }

    #[tokio::test]
    async fn should_err_when_dynamodb_put_fails() {
        let mut fxrates_api = MockFxRatesApiClient::default();
        fxrates_api
            .expect_get_fx_rates()
            .times(Currency::COUNT)
            .returning(|base| {
                let base = *base;
                Box::pin(async move {
                    Ok(FxRatesApiResponse {
                        success: true,
                        base: base.into(),
                        rates: create_complete_rates_for_base(base),
                    })
                })
            });

        let mut repository = MockFxRateDynamoDbRepository::default();
        repository
            .expect_put_fx_rates_record()
            .once()
            .returning(|_| {
                Box::pin(async {
                    Err(SdkError::construction_failure(
                        "Failed to put item in DynamoDB",
                    ))
                })
            });

        let service = FxRateServiceImpl::new(&fxrates_api, &repository);
        let result = service.update_current().await;

        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            FxRateServiceError::SdkErrorPutItem(_)
        ));
    }
}
