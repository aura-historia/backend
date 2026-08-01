use crate::auth::TokenAuthenticator;
use shop_service::use_cases::commands::create_shop::CreateShopUseCase;
use shop_service::use_cases::commands::update_shop::UpdateShopUseCase;
use shop_service::use_cases::queries::get_shop::GetShopUseCase;
use shop_service::use_cases::queries::list_user_partner_shops::ListUserPartnerShopsUseCase;
use shop_service::use_cases::queries::search_shops::SearchShopsUseCase;
use std::sync::Arc;

#[derive(Clone)]
pub struct AppState {
    pub(crate) shops: ShopsState,
}

impl AppState {
    pub fn new(shops: ShopsState) -> Self {
        Self { shops }
    }
}

#[derive(Clone)]
pub struct ShopsState {
    pub(crate) get_shop: Arc<dyn GetShopUseCase>,
    pub(crate) search_shops: Arc<dyn SearchShopsUseCase>,
    pub(crate) create_shop: Arc<dyn CreateShopUseCase>,
    pub(crate) update_shop: Arc<dyn UpdateShopUseCase>,
    pub(crate) list_user_partner_shops: Arc<dyn ListUserPartnerShopsUseCase>,
    pub(crate) authenticator: Arc<dyn TokenAuthenticator>,
}

impl ShopsState {
    pub fn new(
        get_shop: Arc<dyn GetShopUseCase>,
        search_shops: Arc<dyn SearchShopsUseCase>,
        create_shop: Arc<dyn CreateShopUseCase>,
        update_shop: Arc<dyn UpdateShopUseCase>,
        list_user_partner_shops: Arc<dyn ListUserPartnerShopsUseCase>,
        authenticator: Arc<dyn TokenAuthenticator>,
    ) -> Self {
        Self {
            get_shop,
            search_shops,
            create_shop,
            update_shop,
            list_user_partner_shops,
            authenticator,
        }
    }
}
