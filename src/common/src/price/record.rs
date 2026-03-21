use crate::currency::record::CurrencyRecord;
use crate::price::domain::Price;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct PriceRecord {
    pub currency: CurrencyRecord,
    pub amount: u64,
}

impl From<Price> for PriceRecord {
    fn from(domain: Price) -> Self {
        PriceRecord {
            currency: domain.currency.into(),
            amount: domain.monetary_amount.into(),
        }
    }
}

#[cfg(feature = "test-data")]
mod faker {
    use crate::price::{domain::Price, record::PriceRecord};
    use fake::{Dummy, Fake, Faker, RngExt};

    impl Dummy<Faker> for PriceRecord {
        fn dummy_with_rng<R: RngExt + ?Sized>(config: &Faker, rng: &mut R) -> Self {
            config.fake_with_rng::<Price, R>(rng).into()
        }
    }
}
