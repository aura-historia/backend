use crate::use_cases::{
    AdminDecidePartnerShopApplicationUseCase, AdminGetPartnerShopApplicationUseCase,
    AdminListPartnerShopApplicationsUseCase, AdminUpdatePartnerShopApplicationUseCase,
    CreatePartnerShopApplicationUseCase, GetPartnerShopApplicationUseCase,
    ListPartnerShopApplicationsUseCase, ListPartnerShopsUseCase,
    WithdrawPartnerShopApplicationUseCase,
};
use std::sync::Arc;

pub struct ShopPartnerUseCases {
    pub create_application: Arc<dyn CreatePartnerShopApplicationUseCase>,
    pub list_partner_shops: Arc<dyn ListPartnerShopsUseCase>,
    pub list_applications: Arc<dyn ListPartnerShopApplicationsUseCase>,
    pub get_application: Arc<dyn GetPartnerShopApplicationUseCase>,
    pub delete_application: Arc<dyn WithdrawPartnerShopApplicationUseCase>,
    pub admin_list_applications: Arc<dyn AdminListPartnerShopApplicationsUseCase>,
    pub admin_get_application: Arc<dyn AdminGetPartnerShopApplicationUseCase>,
    pub admin_update_application: Arc<dyn AdminUpdatePartnerShopApplicationUseCase>,
    pub admin_decide_application: Arc<dyn AdminDecidePartnerShopApplicationUseCase>,
}

pub struct ShopPartnerUseCasesInput {
    pub create_application: Arc<dyn CreatePartnerShopApplicationUseCase>,
    pub list_partner_shops: Arc<dyn ListPartnerShopsUseCase>,
    pub list_applications: Arc<dyn ListPartnerShopApplicationsUseCase>,
    pub get_application: Arc<dyn GetPartnerShopApplicationUseCase>,
    pub delete_application: Arc<dyn WithdrawPartnerShopApplicationUseCase>,
    pub admin_list_applications: Arc<dyn AdminListPartnerShopApplicationsUseCase>,
    pub admin_get_application: Arc<dyn AdminGetPartnerShopApplicationUseCase>,
    pub admin_update_application: Arc<dyn AdminUpdatePartnerShopApplicationUseCase>,
    pub admin_decide_application: Arc<dyn AdminDecidePartnerShopApplicationUseCase>,
}

impl ShopPartnerUseCases {
    pub fn new(input: ShopPartnerUseCasesInput) -> Self {
        Self {
            create_application: input.create_application,
            list_partner_shops: input.list_partner_shops,
            list_applications: input.list_applications,
            get_application: input.get_application,
            delete_application: input.delete_application,
            admin_list_applications: input.admin_list_applications,
            admin_get_application: input.admin_get_application,
            admin_update_application: input.admin_update_application,
            admin_decide_application: input.admin_decide_application,
        }
    }
}
