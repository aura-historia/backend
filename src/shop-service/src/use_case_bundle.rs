use crate::use_cases::{
    ChangeShopPartnerStatusUseCase, CheckUserPartnerShopUseCase, CreateShopUseCase, GetShopUseCase,
    GrantPartnerShopUseCase, SearchShopsUseCase, UpdateShopUseCase,
};
use std::sync::Arc;

pub struct ShopUseCases {
    pub create: Arc<dyn CreateShopUseCase>,
    pub update: Arc<dyn UpdateShopUseCase>,
    pub change_partner_status: Arc<dyn ChangeShopPartnerStatusUseCase>,
    pub grant_partner_shop: Arc<dyn GrantPartnerShopUseCase>,
    pub check_user_partner_shop: Arc<dyn CheckUserPartnerShopUseCase>,
    pub get: Arc<dyn GetShopUseCase>,
    pub search: Arc<dyn SearchShopsUseCase>,
}

pub struct ShopUseCasesInput {
    pub create: Arc<dyn CreateShopUseCase>,
    pub update: Arc<dyn UpdateShopUseCase>,
    pub change_partner_status: Arc<dyn ChangeShopPartnerStatusUseCase>,
    pub grant_partner_shop: Arc<dyn GrantPartnerShopUseCase>,
    pub check_user_partner_shop: Arc<dyn CheckUserPartnerShopUseCase>,
    pub get: Arc<dyn GetShopUseCase>,
    pub search: Arc<dyn SearchShopsUseCase>,
}

impl ShopUseCases {
    pub fn new(input: ShopUseCasesInput) -> Self {
        Self {
            create: input.create,
            update: input.update,
            change_partner_status: input.change_partner_status,
            grant_partner_shop: input.grant_partner_shop,
            check_user_partner_shop: input.check_user_partner_shop,
            get: input.get,
            search: input.search,
        }
    }
}
