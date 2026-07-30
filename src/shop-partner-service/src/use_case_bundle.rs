use crate::use_cases::{CreatePartnerShopApplicationUseCase, ListPartnerShopsUseCase};
use std::sync::Arc;

pub struct ShopPartnerUseCases {
    pub create_application: Arc<dyn CreatePartnerShopApplicationUseCase>,
    pub list_partner_shops: Arc<dyn ListPartnerShopsUseCase>,
}

pub struct ShopPartnerUseCasesInput {
    pub create_application: Arc<dyn CreatePartnerShopApplicationUseCase>,
    pub list_partner_shops: Arc<dyn ListPartnerShopsUseCase>,
}

impl ShopPartnerUseCases {
    pub fn new(input: ShopPartnerUseCasesInput) -> Self {
        Self {
            create_application: input.create_application,
            list_partner_shops: input.list_partner_shops,
        }
    }
}
