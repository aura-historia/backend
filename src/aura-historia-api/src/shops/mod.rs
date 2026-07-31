pub mod get_shop;

use crate::auth::TokenAuthenticator;
use shop_service::use_cases::queries::get_shop::GetShopUseCase;
use std::sync::Arc;

#[derive(Clone)]
pub struct ShopsState {
    pub get_shop: Arc<dyn GetShopUseCase>,
    pub authenticator: Arc<dyn TokenAuthenticator>,
}

impl ShopsState {
    pub fn new(
        get_shop: Arc<dyn GetShopUseCase>,
        authenticator: Arc<dyn TokenAuthenticator>,
    ) -> Self {
        Self {
            get_shop,
            authenticator,
        }
    }
}
