use crate::auth::TokenAuthenticator;
use shop_partner_service::use_cases::{
    AdminDecidePartnerShopApplicationUseCase, AdminGetPartnerShopApplicationUseCase,
    AdminListPartnerShopApplicationsUseCase, AdminUpdatePartnerShopApplicationUseCase,
    CreatePartnerShopApplicationUseCase, GetPartnerShopApplicationUseCase,
    ListPartnerShopApplicationsUseCase, WithdrawPartnerShopApplicationUseCase,
};
use shop_service::use_cases::commands::create_shop::CreateShopUseCase;
use shop_service::use_cases::commands::update_shop::UpdateShopUseCase;
use shop_service::use_cases::queries::get_shop::GetShopUseCase;
use shop_service::use_cases::queries::list_user_partner_shops::ListUserPartnerShopsUseCase;
use shop_service::use_cases::queries::search_shops::SearchShopsUseCase;
use std::sync::Arc;
use user_service::use_cases::commands::change_user_role::ChangeUserRoleUseCase;
use user_service::use_cases::commands::change_user_tier::ChangeUserTierUseCase;
use user_service::use_cases::commands::create_access_token::CreateAccessTokenUseCase;
use user_service::use_cases::commands::delete_access_token::DeleteAccessTokenUseCase;
use user_service::use_cases::commands::delete_user::DeleteUserUseCase;
use user_service::use_cases::commands::update_access_token::UpdateAccessTokenUseCase;
use user_service::use_cases::commands::update_user_profile::UpdateUserProfileUseCase;
use user_service::use_cases::queries::admin_get_user::AdminGetUserUseCase;
use user_service::use_cases::queries::get_access_token::GetAccessTokenUseCase;
use user_service::use_cases::queries::get_own_user::GetOwnUserUseCase;
use user_service::use_cases::queries::list_access_tokens::ListAccessTokensUseCase;
use user_service::use_cases::queries::search_users::SearchUsersUseCase;
use watchlist_service::use_cases::{
    ListWatchlistUseCase, UnwatchProductUseCase, UpdateWatchlistProductUseCase, WatchProductUseCase,
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
    pub(crate) get_own_user: Arc<dyn GetOwnUserUseCase>,
    pub(crate) admin_get_user: Arc<dyn AdminGetUserUseCase>,
    pub(crate) search_users: Arc<dyn SearchUsersUseCase>,
    pub(crate) update_user_profile: Arc<dyn UpdateUserProfileUseCase>,
    pub(crate) change_user_role: Arc<dyn ChangeUserRoleUseCase>,
    pub(crate) change_user_tier: Arc<dyn ChangeUserTierUseCase>,
    pub(crate) delete_user: Arc<dyn DeleteUserUseCase>,
    pub(crate) create_access_token: Arc<dyn CreateAccessTokenUseCase>,
    pub(crate) list_access_tokens: Arc<dyn ListAccessTokensUseCase>,
    pub(crate) get_access_token: Arc<dyn GetAccessTokenUseCase>,
    pub(crate) update_access_token: Arc<dyn UpdateAccessTokenUseCase>,
    pub(crate) delete_access_token: Arc<dyn DeleteAccessTokenUseCase>,
    pub(crate) authenticator: Arc<dyn TokenAuthenticator>,
}

impl UsersState {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        get_own_user: Arc<dyn GetOwnUserUseCase>,
        admin_get_user: Arc<dyn AdminGetUserUseCase>,
        search_users: Arc<dyn SearchUsersUseCase>,
        update_user_profile: Arc<dyn UpdateUserProfileUseCase>,
        change_user_role: Arc<dyn ChangeUserRoleUseCase>,
        change_user_tier: Arc<dyn ChangeUserTierUseCase>,
        delete_user: Arc<dyn DeleteUserUseCase>,
        create_access_token: Arc<dyn CreateAccessTokenUseCase>,
        list_access_tokens: Arc<dyn ListAccessTokensUseCase>,
        get_access_token: Arc<dyn GetAccessTokenUseCase>,
        update_access_token: Arc<dyn UpdateAccessTokenUseCase>,
        delete_access_token: Arc<dyn DeleteAccessTokenUseCase>,
        authenticator: Arc<dyn TokenAuthenticator>,
    ) -> Self {
        Self {
            get_own_user,
            admin_get_user,
            search_users,
            update_user_profile,
            change_user_role,
            change_user_tier,
            delete_user,
            create_access_token,
            list_access_tokens,
            get_access_token,
            update_access_token,
            delete_access_token,
            authenticator,
        }
    }
}

#[derive(Clone)]
pub struct WatchlistState {
    pub(crate) list_watchlist: Arc<dyn ListWatchlistUseCase>,
    pub(crate) watch_product: Arc<dyn WatchProductUseCase>,
    pub(crate) update_watchlist_product: Arc<dyn UpdateWatchlistProductUseCase>,
    pub(crate) unwatch_product: Arc<dyn UnwatchProductUseCase>,
    pub(crate) authenticator: Arc<dyn TokenAuthenticator>,
}

impl WatchlistState {
    pub fn new(
        list_watchlist: Arc<dyn ListWatchlistUseCase>,
        watch_product: Arc<dyn WatchProductUseCase>,
        update_watchlist_product: Arc<dyn UpdateWatchlistProductUseCase>,
        unwatch_product: Arc<dyn UnwatchProductUseCase>,
        authenticator: Arc<dyn TokenAuthenticator>,
    ) -> Self {
        Self {
            list_watchlist,
            watch_product,
            update_watchlist_product,
            unwatch_product,
            authenticator,
        }
    }
}

#[derive(Clone)]
pub struct PartnerApplicationsState {
    pub(crate) create: Arc<dyn CreatePartnerShopApplicationUseCase>,
    pub(crate) list: Arc<dyn ListPartnerShopApplicationsUseCase>,
    pub(crate) get: Arc<dyn GetPartnerShopApplicationUseCase>,
    pub(crate) delete: Arc<dyn WithdrawPartnerShopApplicationUseCase>,
    pub(crate) admin_list: Arc<dyn AdminListPartnerShopApplicationsUseCase>,
    pub(crate) admin_get: Arc<dyn AdminGetPartnerShopApplicationUseCase>,
    pub(crate) admin_update: Arc<dyn AdminUpdatePartnerShopApplicationUseCase>,
    pub(crate) admin_decide: Arc<dyn AdminDecidePartnerShopApplicationUseCase>,
    pub(crate) authenticator: Arc<dyn TokenAuthenticator>,
}

impl PartnerApplicationsState {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        create: Arc<dyn CreatePartnerShopApplicationUseCase>,
        list: Arc<dyn ListPartnerShopApplicationsUseCase>,
        get: Arc<dyn GetPartnerShopApplicationUseCase>,
        delete: Arc<dyn WithdrawPartnerShopApplicationUseCase>,
        admin_list: Arc<dyn AdminListPartnerShopApplicationsUseCase>,
        admin_get: Arc<dyn AdminGetPartnerShopApplicationUseCase>,
        admin_update: Arc<dyn AdminUpdatePartnerShopApplicationUseCase>,
        admin_decide: Arc<dyn AdminDecidePartnerShopApplicationUseCase>,
        authenticator: Arc<dyn TokenAuthenticator>,
    ) -> Self {
        Self {
            create,
            list,
            get,
            delete,
            admin_list,
            admin_get,
            admin_update,
            admin_decide,
            authenticator,
        }
    }
}
