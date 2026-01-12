use crate::{
    dynamodb::{
        record::{FxRatesRecord, mk_pk, mk_sk},
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
    SdkErrorPuttItem(#[from] SdkError<PutItemError>),

    #[error("FxRatesNotExists: The FxRatesRecord does not exist")]
    FxRatesNotExists,

    #[error("FxratesApiError: The reponse contained success=false")]
    FxratesApiError,

    #[error("MissingFxRate: Missing FxRate to exchang from '{0}' to '{1}'")]
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

        let fx_rates_record = FxRatesRecord {
            pk: mk_pk().to_owned(),
            sk: mk_sk().to_owned(),
            eur_gbp: *rates.get(&(Currency::Eur, Currency::Gbp)).ok_or(
                FxRateServiceError::MissingFxRate(Currency::Eur, Currency::Gbp),
            )?,
            eur_usd: *rates.get(&(Currency::Eur, Currency::Usd)).ok_or(
                FxRateServiceError::MissingFxRate(Currency::Eur, Currency::Usd),
            )?,
            eur_aud: *rates.get(&(Currency::Eur, Currency::Aud)).ok_or(
                FxRateServiceError::MissingFxRate(Currency::Eur, Currency::Aud),
            )?,
            eur_cad: *rates.get(&(Currency::Eur, Currency::Cad)).ok_or(
                FxRateServiceError::MissingFxRate(Currency::Eur, Currency::Cad),
            )?,
            eur_nzd: *rates.get(&(Currency::Eur, Currency::Nzd)).ok_or(
                FxRateServiceError::MissingFxRate(Currency::Eur, Currency::Nzd),
            )?,

            gbp_eur: *rates.get(&(Currency::Gbp, Currency::Eur)).ok_or(
                FxRateServiceError::MissingFxRate(Currency::Gbp, Currency::Eur),
            )?,
            gbp_usd: *rates.get(&(Currency::Gbp, Currency::Usd)).ok_or(
                FxRateServiceError::MissingFxRate(Currency::Gbp, Currency::Usd),
            )?,
            gbp_aud: *rates.get(&(Currency::Gbp, Currency::Aud)).ok_or(
                FxRateServiceError::MissingFxRate(Currency::Gbp, Currency::Aud),
            )?,
            gbp_cad: *rates.get(&(Currency::Gbp, Currency::Cad)).ok_or(
                FxRateServiceError::MissingFxRate(Currency::Gbp, Currency::Cad),
            )?,
            gbp_nzd: *rates.get(&(Currency::Gbp, Currency::Nzd)).ok_or(
                FxRateServiceError::MissingFxRate(Currency::Gbp, Currency::Nzd),
            )?,

            usd_eur: *rates.get(&(Currency::Usd, Currency::Eur)).ok_or(
                FxRateServiceError::MissingFxRate(Currency::Usd, Currency::Eur),
            )?,
            usd_gbp: *rates.get(&(Currency::Usd, Currency::Gbp)).ok_or(
                FxRateServiceError::MissingFxRate(Currency::Usd, Currency::Gbp),
            )?,
            usd_aud: *rates.get(&(Currency::Usd, Currency::Aud)).ok_or(
                FxRateServiceError::MissingFxRate(Currency::Usd, Currency::Aud),
            )?,
            usd_cad: *rates.get(&(Currency::Usd, Currency::Cad)).ok_or(
                FxRateServiceError::MissingFxRate(Currency::Usd, Currency::Cad),
            )?,
            usd_nzd: *rates.get(&(Currency::Usd, Currency::Nzd)).ok_or(
                FxRateServiceError::MissingFxRate(Currency::Usd, Currency::Nzd),
            )?,

            aud_eur: *rates.get(&(Currency::Aud, Currency::Eur)).ok_or(
                FxRateServiceError::MissingFxRate(Currency::Aud, Currency::Eur),
            )?,
            aud_gbp: *rates.get(&(Currency::Aud, Currency::Gbp)).ok_or(
                FxRateServiceError::MissingFxRate(Currency::Aud, Currency::Gbp),
            )?,
            aud_usd: *rates.get(&(Currency::Aud, Currency::Usd)).ok_or(
                FxRateServiceError::MissingFxRate(Currency::Aud, Currency::Usd),
            )?,
            aud_cad: *rates.get(&(Currency::Aud, Currency::Cad)).ok_or(
                FxRateServiceError::MissingFxRate(Currency::Aud, Currency::Cad),
            )?,
            aud_nzd: *rates.get(&(Currency::Aud, Currency::Nzd)).ok_or(
                FxRateServiceError::MissingFxRate(Currency::Aud, Currency::Nzd),
            )?,

            cad_eur: *rates.get(&(Currency::Cad, Currency::Eur)).ok_or(
                FxRateServiceError::MissingFxRate(Currency::Cad, Currency::Eur),
            )?,
            cad_gbp: *rates.get(&(Currency::Cad, Currency::Gbp)).ok_or(
                FxRateServiceError::MissingFxRate(Currency::Cad, Currency::Gbp),
            )?,
            cad_usd: *rates.get(&(Currency::Cad, Currency::Usd)).ok_or(
                FxRateServiceError::MissingFxRate(Currency::Cad, Currency::Usd),
            )?,
            cad_aud: *rates.get(&(Currency::Cad, Currency::Aud)).ok_or(
                FxRateServiceError::MissingFxRate(Currency::Cad, Currency::Aud),
            )?,
            cad_nzd: *rates.get(&(Currency::Cad, Currency::Nzd)).ok_or(
                FxRateServiceError::MissingFxRate(Currency::Cad, Currency::Nzd),
            )?,

            nzd_eur: *rates.get(&(Currency::Nzd, Currency::Eur)).ok_or(
                FxRateServiceError::MissingFxRate(Currency::Nzd, Currency::Eur),
            )?,
            nzd_gbp: *rates.get(&(Currency::Nzd, Currency::Gbp)).ok_or(
                FxRateServiceError::MissingFxRate(Currency::Nzd, Currency::Gbp),
            )?,
            nzd_usd: *rates.get(&(Currency::Nzd, Currency::Usd)).ok_or(
                FxRateServiceError::MissingFxRate(Currency::Nzd, Currency::Usd),
            )?,
            nzd_aud: *rates.get(&(Currency::Nzd, Currency::Aud)).ok_or(
                FxRateServiceError::MissingFxRate(Currency::Nzd, Currency::Aud),
            )?,
            nzd_cad: *rates.get(&(Currency::Nzd, Currency::Cad)).ok_or(
                FxRateServiceError::MissingFxRate(Currency::Nzd, Currency::Cad),
            )?,
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
        dynamodb::repository::MockFxRateDynamoDbRepository,
        fxratesapi::{FxRatesApiResponse, MockFxRatesApiClient},
        service::{FxRateService, FxRateServiceError, FxRateServiceImpl},
    };
    use aws_sdk_dynamodb::{error::SdkError, operation::put_item::PutItemOutput};
    use common::{currency::data::CurrencyData, currency::domain::Currency};
    use std::collections::HashMap;
    use strum::{EnumCount, IntoEnumIterator};

    fn create_complete_rates_for_base(base: Currency) -> HashMap<CurrencyData, f32> {
        let mut rates = HashMap::new();
        for currency in Currency::iter() {
            if currency != base {
                let rate = match (base, currency) {
                    (Currency::Eur, Currency::Usd) => 1.1,
                    (Currency::Eur, Currency::Gbp) => 0.85,
                    (Currency::Eur, Currency::Aud) => 1.6,
                    (Currency::Eur, Currency::Cad) => 1.5,
                    (Currency::Eur, Currency::Nzd) => 1.7,
                    (Currency::Usd, Currency::Eur) => 0.91,
                    (Currency::Usd, Currency::Gbp) => 0.77,
                    (Currency::Usd, Currency::Aud) => 1.45,
                    (Currency::Usd, Currency::Cad) => 1.36,
                    (Currency::Usd, Currency::Nzd) => 1.55,
                    (Currency::Gbp, Currency::Eur) => 1.18,
                    (Currency::Gbp, Currency::Usd) => 1.3,
                    (Currency::Gbp, Currency::Aud) => 1.88,
                    (Currency::Gbp, Currency::Cad) => 1.76,
                    (Currency::Gbp, Currency::Nzd) => 2.0,
                    (Currency::Aud, Currency::Eur) => 0.63,
                    (Currency::Aud, Currency::Gbp) => 0.53,
                    (Currency::Aud, Currency::Usd) => 0.69,
                    (Currency::Aud, Currency::Cad) => 0.94,
                    (Currency::Aud, Currency::Nzd) => 1.06,
                    (Currency::Cad, Currency::Eur) => 0.67,
                    (Currency::Cad, Currency::Gbp) => 0.57,
                    (Currency::Cad, Currency::Usd) => 0.74,
                    (Currency::Cad, Currency::Aud) => 1.06,
                    (Currency::Cad, Currency::Nzd) => 1.13,
                    (Currency::Nzd, Currency::Eur) => 0.59,
                    (Currency::Nzd, Currency::Gbp) => 0.5,
                    (Currency::Nzd, Currency::Usd) => 0.65,
                    (Currency::Nzd, Currency::Aud) => 0.94,
                    (Currency::Nzd, Currency::Cad) => 0.88,
                    _ => 1.0,
                };
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
        assert!(record.eur_usd > 0);
        assert!(record.gbp_eur > 0);
    }

    #[tokio::test]
    async fn should_err_when_fxrates_api_not_succesful() {
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
            FxRateServiceError::SdkErrorPuttItem(_)
        ));
    }
}
