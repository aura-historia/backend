use crate::core::partner_shop_application::{PartnerShopApplicationPayload, PartnerShopApplicationPayloadInfo};
use common::{domain::Domain, shop_name::ShopName, user_id::UserId};
use shop::core::shop_type::ShopType;
use std::collections::HashSet;
use url::Url;

#[derive(Debug, Clone, PartialEq)]
pub struct CreatePartnerShopApplicationCommand {
    pub applicant_user_id: UserId,
    pub payload: PartnerShopApplicationPayload,
}

#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(Debug, Clone, PartialEq, Default)]
pub struct UpdatePartnerShopApplicationCommand {
    pub shop_name: Option<ShopName>,
    pub shop_type: Option<ShopType>,
    pub shop_domains: Option<HashSet<Domain>>,
    pub shop_image: Option<Url>,
}

impl UpdatePartnerShopApplicationCommand {
    pub fn is_empty(&self) -> bool {
        self.shop_name.is_none()
            && self.shop_type.is_none()
            && self.shop_domains.is_none()
            && self.shop_image.is_none()
    }

    pub fn has_payload_info_update(&self) -> bool {
        self.shop_name.is_some()
            || self.shop_type.is_some()
            || self.shop_domains.is_some()
            || self.shop_image.is_some()
    }

    pub fn into_payload_info(self) -> Option<PartnerShopApplicationPayloadInfo> {
        if !self.has_payload_info_update() {
            return None;
        }
        Some(PartnerShopApplicationPayloadInfo {
            shop_name: self.shop_name,
            shop_type: self.shop_type,
            shop_domains: self.shop_domains,
            shop_image: self.shop_image,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    Approve,
    Reject,
}

#[cfg(feature = "test-data")]
mod faker {
    use super::*;
    use fake::{Dummy, Fake, Faker, RngExt};

    impl Dummy<Faker> for CreatePartnerShopApplicationCommand {
        fn dummy_with_rng<R: RngExt + ?Sized>(config: &Faker, rng: &mut R) -> Self {
            CreatePartnerShopApplicationCommand {
                applicant_user_id: config.fake_with_rng(rng),
                payload: config.fake_with_rng(rng),
            }
        }
    }

    impl Dummy<Faker> for Decision {
        fn dummy_with_rng<R: RngExt + ?Sized>(config: &Faker, rng: &mut R) -> Self {
            if config.fake_with_rng::<bool, R>(rng) {
                Decision::Approve
            } else {
                Decision::Reject
            }
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use fake::{Fake, Faker};

        #[test]
        fn should_fake_create_partner_shop_application_command() {
            let _ = Faker.fake::<CreatePartnerShopApplicationCommand>();
        }

        #[test]
        fn should_fake_update_partner_shop_application_command() {
            let _ = Faker.fake::<UpdatePartnerShopApplicationCommand>();
        }

        #[test]
        fn should_be_empty_when_default() {
            let cmd = UpdatePartnerShopApplicationCommand::default();
            assert!(cmd.is_empty());
            assert!(!cmd.has_payload_info_update());
        }

        #[test]
        fn should_not_be_empty_when_new_shop_name_is_set() {
            let cmd = UpdatePartnerShopApplicationCommand {
                shop_name: Some(ShopName::from("Test".to_string())),
                ..Default::default()
            };
            assert!(!cmd.is_empty());
            assert!(cmd.has_payload_info_update());
        }

        #[test]
        fn should_fake_decision() {
            let _ = Faker.fake::<Decision>();
        }
    }
}
