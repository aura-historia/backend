use crate::core::partner_shop_application_id::PartnerShopApplicationId;
use common::{shop_id::ShopId, user_id::UserId};
use time::OffsetDateTime;

#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PartnerShopApplicationState {
    Submitted,
    InReview,
    Rejected,
    Approved,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PartnerShopApplication {
    pub id: PartnerShopApplicationId,
    pub state: PartnerShopApplicationState,
    pub applicant_user_id: UserId,
    pub payload: PartnerShopApplicationPayload,
    pub created: OffsetDateTime,
    pub updated: OffsetDateTime,
}

#[derive(Debug, Clone, PartialEq)]
pub enum PartnerShopApplicationPayload {
    Existing(ShopId),
    New(CreateShopCommand),
}

pub use shop::service::command::CreateShopCommand;

#[derive(Debug, Clone, PartialEq)]
pub struct CreatePartnerShopApplication {
    pub applicant_user_id: UserId,
    pub payload: PartnerShopApplicationPayload,
}

#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(Debug, Clone, PartialEq, Default)]
pub struct UpdatePartnerShopApplication {
    pub state: Option<PartnerShopApplicationState>,
}

impl UpdatePartnerShopApplication {
    pub fn is_empty(&self) -> bool {
        self.state.is_none()
    }
}

#[cfg(feature = "test-data")]
mod faker {
    use super::*;
    use fake::{Dummy, Fake, Faker, RngExt};

    impl Dummy<Faker> for PartnerShopApplication {
        fn dummy_with_rng<R: RngExt + ?Sized>(config: &Faker, rng: &mut R) -> Self {
            let created = OffsetDateTime::now_utc();
            PartnerShopApplication {
                id: PartnerShopApplicationId::new(),
                state: config.fake_with_rng(rng),
                applicant_user_id: config.fake_with_rng(rng),
                payload: config.fake_with_rng(rng),
                created,
                updated: created,
            }
        }
    }

    impl Dummy<Faker> for PartnerShopApplicationPayload {
        fn dummy_with_rng<R: RngExt + ?Sized>(config: &Faker, rng: &mut R) -> Self {
            if config.fake_with_rng::<bool, R>(rng) {
                PartnerShopApplicationPayload::Existing(config.fake_with_rng(rng))
            } else {
                PartnerShopApplicationPayload::New(config.fake_with_rng(rng))
            }
        }
    }

    impl Dummy<Faker> for CreatePartnerShopApplication {
        fn dummy_with_rng<R: RngExt + ?Sized>(config: &Faker, rng: &mut R) -> Self {
            CreatePartnerShopApplication {
                applicant_user_id: config.fake_with_rng(rng),
                payload: config.fake_with_rng(rng),
            }
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use fake::{Fake, Faker};

        #[test]
        fn should_fake_partner_shop_application() {
            let _ = Faker.fake::<PartnerShopApplication>();
        }

        #[test]
        fn should_fake_partner_shop_application_payload() {
            let _ = Faker.fake::<PartnerShopApplicationPayload>();
        }

        #[test]
        fn should_fake_create_partner_shop_application() {
            let _ = Faker.fake::<CreatePartnerShopApplication>();
        }

        #[test]
        fn should_fake_update_partner_shop_application() {
            let _ = Faker.fake::<UpdatePartnerShopApplication>();
        }

        #[test]
        fn should_fake_partner_shop_application_state() {
            let _ = Faker.fake::<PartnerShopApplicationState>();
        }
    }
}
