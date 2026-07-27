use crate::service::use_cases::{
    ChangeShopPartnerStatusUseCase, CreateShopUseCase, GetShopUseCase, SearchShopsUseCase,
    UpdateShopUseCase,
};
use std::sync::Arc;

pub struct ShopUseCases {
    pub create: Arc<dyn CreateShopUseCase>,
    pub update: Arc<dyn UpdateShopUseCase>,
    pub change_partner_status: Arc<dyn ChangeShopPartnerStatusUseCase>,
    pub get: Arc<dyn GetShopUseCase>,
    pub search: Arc<dyn SearchShopsUseCase>,
}

impl ShopUseCases {
    pub fn new(
        create: Arc<dyn CreateShopUseCase>,
        update: Arc<dyn UpdateShopUseCase>,
        change_partner_status: Arc<dyn ChangeShopPartnerStatusUseCase>,
        get: Arc<dyn GetShopUseCase>,
        search: Arc<dyn SearchShopsUseCase>,
    ) -> Self {
        Self {
            create,
            update,
            change_partner_status,
            get,
            search,
        }
    }
}
