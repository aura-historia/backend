use crate::auth::TokenAuthenticator;
use shop_partner_service::use_cases::{
    AdminDecidePartnerShopApplicationUseCase, AdminGetPartnerShopApplicationUseCase,
    AdminListPartnerShopApplicationsUseCase, AdminUpdatePartnerShopApplicationUseCase,
    CreatePartnerShopApplicationUseCase, DeletePartnerShopApplicationUseCase,
    GetPartnerShopApplicationUseCase, ListPartnerShopApplicationsUseCase,
    UpdatePartnerShopApplicationUseCase,
};
use shop_service::use_cases::commands::create_shop::CreateShopUseCase;
use shop_service::use_cases::commands::update_shop::UpdateShopUseCase;
use shop_service::use_cases::queries::get_shop::GetShopUseCase;
use shop_service::use_cases::queries::list_user_partner_shops::ListUserPartnerShopsUseCase;
use shop_service::use_cases::queries::search_shops::SearchShopsUseCase;
use std::sync::Arc;
use user_service::use_cases::commands::create_access_token::CreateAccessTokenUseCase;
use user_service::use_cases::commands::delete_access_token::DeleteAccessTokenUseCase;
use user_service::use_cases::commands::delete_user::DeleteUserUseCase;
use user_service::use_cases::commands::update_access_token::UpdateAccessTokenUseCase;
use user_service::use_cases::commands::update_user::UpdateUserUseCase;
use user_service::use_cases::queries::check_user_admin::CheckUserAdminUseCase;
use user_service::use_cases::queries::get_access_token::GetAccessTokenUseCase;
use user_service::use_cases::queries::get_user::GetUserUseCase;
use user_service::use_cases::queries::list_access_tokens::ListAccessTokensUseCase;
use user_service::use_cases::queries::search_users::SearchUsersUseCase;
use watchlist_service::use_cases::{
    DeleteWatchlistProductUseCase, ListWatchlistUseCase, UpdateWatchlistProductUseCase,
    WatchProductUseCase,
};

#[derive(Clone)]
pub struct AppState {
    pub(crate) shops: ShopsState,
    pub(crate) users: Option<UsersState>,
    pub(crate) watchlist: Option<WatchlistState>,
    pub(crate) partner_applications: Option<PartnerApplicationsState>,
}

impl AppState {
    pub fn new(
        shops: ShopsState,
        users: UsersState,
        watchlist: WatchlistState,
        partner_applications: PartnerApplicationsState,
    ) -> Self {
        Self {
            shops,
            users: Some(users),
            watchlist: Some(watchlist),
            partner_applications: Some(partner_applications),
        }
    }

    pub fn with_shops_only(shops: ShopsState) -> Self {
        Self {
            shops,
            users: None,
            watchlist: None,
            partner_applications: None,
        }
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

#[derive(Clone)]
pub struct UsersState {
    pub(crate) get_user: Arc<dyn GetUserUseCase>,
    pub(crate) search_users: Arc<dyn SearchUsersUseCase>,
    pub(crate) update_user: Arc<dyn UpdateUserUseCase>,
    pub(crate) delete_user: Arc<dyn DeleteUserUseCase>,
    pub(crate) check_user_admin: Arc<dyn CheckUserAdminUseCase>,
    pub(crate) create_access_token: Arc<dyn CreateAccessTokenUseCase>,
    pub(crate) list_access_tokens: Arc<dyn ListAccessTokensUseCase>,
    pub(crate) get_access_token: Arc<dyn GetAccessTokenUseCase>,
    pub(crate) update_access_token: Arc<dyn UpdateAccessTokenUseCase>,
    pub(crate) delete_access_token: Arc<dyn DeleteAccessTokenUseCase>,
    pub(crate) authenticator: Arc<dyn TokenAuthenticator>,
}

#[derive(Clone)]
pub struct WatchlistState {
    pub(crate) list_watchlist: Arc<dyn ListWatchlistUseCase>,
    pub(crate) watch_product: Arc<dyn WatchProductUseCase>,
    pub(crate) update_watchlist_product: Arc<dyn UpdateWatchlistProductUseCase>,
    pub(crate) delete_watchlist_product: Arc<dyn DeleteWatchlistProductUseCase>,
    pub(crate) authenticator: Arc<dyn TokenAuthenticator>,
}

#[derive(Clone)]
pub struct PartnerApplicationsState {
    pub(crate) create: Arc<dyn CreatePartnerShopApplicationUseCase>,
    pub(crate) list: Arc<dyn ListPartnerShopApplicationsUseCase>,
    pub(crate) get: Arc<dyn GetPartnerShopApplicationUseCase>,
    pub(crate) update: Arc<dyn UpdatePartnerShopApplicationUseCase>,
    pub(crate) delete: Arc<dyn DeletePartnerShopApplicationUseCase>,
    pub(crate) admin_list: Arc<dyn AdminListPartnerShopApplicationsUseCase>,
    pub(crate) admin_get: Arc<dyn AdminGetPartnerShopApplicationUseCase>,
    pub(crate) admin_update: Arc<dyn AdminUpdatePartnerShopApplicationUseCase>,
    pub(crate) admin_decide: Arc<dyn AdminDecidePartnerShopApplicationUseCase>,
    pub(crate) authenticator: Arc<dyn TokenAuthenticator>,
}
