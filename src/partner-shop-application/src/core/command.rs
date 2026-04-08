use crate::core::{
    partner_shop_application::{PartnerShopApplicationPayload, PartnerShopApplicationPayloadInfo},
    partner_shop_application_state::PartnerShopApplicationState,
};
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
    pub state: Option<PartnerShopApplicationState>,
    pub new_shop_name: Option<ShopName>,
    pub new_shop_type: Option<ShopType>,
    pub new_shop_domains: Option<HashSet<Domain>>,
    pub new_shop_image: Option<Url>,
}

impl UpdatePartnerShopApplicationCommand {
    pub fn is_empty(&self) -> bool {
        self.state.is_none()
            && self.new_shop_name.is_none()
            && self.new_shop_type.is_none()
            && self.new_shop_domains.is_none()
            && self.new_shop_image.is_none()
    }

    pub fn has_payload_info_update(&self) -> bool {
        self.new_shop_name.is_some()
            || self.new_shop_type.is_some()
            || self.new_shop_domains.is_some()
            || self.new_shop_image.is_some()
    }

    pub fn into_payload_info(self) -> Option<PartnerShopApplicationPayloadInfo> {
        if !self.has_payload_info_update() {
            return None;
        }
        Some(PartnerShopApplicationPayloadInfo {
            new_shop_name: self.new_shop_name,
            new_shop_type: self.new_shop_type,
            new_shop_domains: self.new_shop_domains,
            new_shop_image: self.new_shop_image,
        })
    }
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
        fn should_not_be_empty_when_state_is_set() {
            let cmd = UpdatePartnerShopApplicationCommand {
                state: Some(PartnerShopApplicationState::Approved),
                ..Default::default()
            };
            assert!(!cmd.is_empty());
        }

        #[test]
        fn should_not_be_empty_when_new_shop_name_is_set() {
            let cmd = UpdatePartnerShopApplicationCommand {
                new_shop_name: Some(ShopName::from("Test".to_string())),
                ..Default::default()
            };
            assert!(!cmd.is_empty());
            assert!(cmd.has_payload_info_update());
        }
    }
}
