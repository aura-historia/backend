use common::{
    currency::domain::Currency,
    price::domain::{FX_RATE_SCALE, FxRate, MonetaryAmount, MonetaryAmountOverflowError, Rate},
};
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FxRatesRecord {
    pub pk: String,
    pub sk: String,

    pub eur_gbp: Rate,
    pub eur_usd: Rate,
    pub eur_aud: Rate,
    pub eur_cad: Rate,
    pub eur_nzd: Rate,

    pub gbp_eur: Rate,
    pub gbp_usd: Rate,
    pub gbp_aud: Rate,
    pub gbp_cad: Rate,
    pub gbp_nzd: Rate,

    pub usd_eur: Rate,
    pub usd_gbp: Rate,
    pub usd_aud: Rate,
    pub usd_cad: Rate,
    pub usd_nzd: Rate,

    pub aud_eur: Rate,
    pub aud_gbp: Rate,
    pub aud_usd: Rate,
    pub aud_cad: Rate,
    pub aud_nzd: Rate,

    pub cad_eur: Rate,
    pub cad_gbp: Rate,
    pub cad_usd: Rate,
    pub cad_aud: Rate,
    pub cad_nzd: Rate,

    pub nzd_eur: Rate,
    pub nzd_gbp: Rate,
    pub nzd_usd: Rate,
    pub nzd_aud: Rate,
    pub nzd_cad: Rate,

    #[serde(with = "time::serde::rfc3339")]
    pub timestamp: OffsetDateTime,
}

pub fn mk_pk() -> &'static str {
    "global#fx_rate"
}

pub fn mk_sk() -> &'static str {
    "fx_rate#details"
}

impl FxRatesRecord {
    fn get_rate(&self, from: Currency, to: Currency) -> Rate {
        match (from, to) {
            (Currency::Eur, Currency::Eur) => FX_RATE_SCALE,
            (Currency::Eur, Currency::Usd) => self.eur_usd,
            (Currency::Eur, Currency::Gbp) => self.eur_gbp,
            (Currency::Eur, Currency::Aud) => self.eur_aud,
            (Currency::Eur, Currency::Cad) => self.eur_cad,
            (Currency::Eur, Currency::Nzd) => self.eur_nzd,

            (Currency::Usd, Currency::Eur) => self.usd_eur,
            (Currency::Usd, Currency::Gbp) => self.usd_gbp,
            (Currency::Usd, Currency::Aud) => self.usd_aud,
            (Currency::Usd, Currency::Cad) => self.usd_cad,
            (Currency::Usd, Currency::Nzd) => self.usd_nzd,
            (Currency::Usd, Currency::Usd) => FX_RATE_SCALE,

            (Currency::Gbp, Currency::Eur) => self.gbp_eur,
            (Currency::Gbp, Currency::Usd) => self.gbp_usd,
            (Currency::Gbp, Currency::Aud) => self.gbp_aud,
            (Currency::Gbp, Currency::Cad) => self.gbp_cad,
            (Currency::Gbp, Currency::Nzd) => self.gbp_nzd,
            (Currency::Gbp, Currency::Gbp) => FX_RATE_SCALE,

            (Currency::Aud, Currency::Eur) => self.aud_eur,
            (Currency::Aud, Currency::Usd) => self.aud_usd,
            (Currency::Aud, Currency::Gbp) => self.aud_gbp,
            (Currency::Aud, Currency::Cad) => self.aud_cad,
            (Currency::Aud, Currency::Nzd) => self.aud_nzd,
            (Currency::Aud, Currency::Aud) => FX_RATE_SCALE,

            (Currency::Cad, Currency::Eur) => self.cad_eur,
            (Currency::Cad, Currency::Usd) => self.cad_usd,
            (Currency::Cad, Currency::Gbp) => self.cad_gbp,
            (Currency::Cad, Currency::Aud) => self.cad_aud,
            (Currency::Cad, Currency::Nzd) => self.cad_nzd,
            (Currency::Cad, Currency::Cad) => FX_RATE_SCALE,

            (Currency::Nzd, Currency::Eur) => self.nzd_eur,
            (Currency::Nzd, Currency::Usd) => self.nzd_usd,
            (Currency::Nzd, Currency::Gbp) => self.nzd_gbp,
            (Currency::Nzd, Currency::Aud) => self.nzd_aud,
            (Currency::Nzd, Currency::Cad) => self.nzd_cad,
            (Currency::Nzd, Currency::Nzd) => FX_RATE_SCALE,
        }
    }
}

impl FxRate for FxRatesRecord {
    fn exchange(
        &self,
        from_currency: Currency,
        to_currency: Currency,
        from_amount: MonetaryAmount,
    ) -> Result<MonetaryAmount, MonetaryAmountOverflowError> {
        let rate = self.get_rate(from_currency, to_currency);

        // Half-Up Rounding
        let numerator = (*from_amount)
            .checked_mul(rate)
            .ok_or(MonetaryAmountOverflowError)?;
        let half = FX_RATE_SCALE / 2;
        let converted = (numerator + half) / FX_RATE_SCALE;

        Ok(MonetaryAmount::from(converted))
    }
}
