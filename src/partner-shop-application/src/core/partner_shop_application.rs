use crate::core::{
    partner_shop_application_id::PartnerShopApplicationId,
    partner_shop_application_state::PartnerShopApplicationState,
};
use common::{
    domain::Domain, execution_state::ExecutionState, shop_id::ShopId, shop_name::ShopName,
    user_id::UserId,
};
use shop::core::shop_type::ShopType;
use std::collections::HashSet;
use time::OffsetDateTime;
use url::Url;

pub use shop::service::command::CreateShopCommand;

#[derive(Debug, Clone, PartialEq)]
pub struct PartnerShopApplication {
    pub id: PartnerShopApplicationId,
    pub business_state: PartnerShopApplicationState,
    pub execution_state: ExecutionState,
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

#[derive(Debug, Clone, PartialEq)]
pub struct PartnerShopApplicationPayloadInfo {
    pub shop_name: Option<ShopName>,
    pub shop_type: Option<ShopType>,
    pub shop_domains: Option<HashSet<Domain>>,
    pub shop_image: Option<Url>,
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
                business_state: config.fake_with_rng(rng),
                execution_state: config.fake_with_rng(rng),
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

    #[cfg(test)]
    mod tests {
        use super::*;
        use crate::core::partner_shop_application_state::PartnerShopApplicationState;
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
        fn should_fake_partner_shop_application_state() {
            let _ = Faker.fake::<PartnerShopApplicationState>();
        }
    }
}
