use crate::auth::TokenAuthenticator;
use common::error::boxed::static_error;
use common::operation_context::OperationContext;
use shop_service::use_cases::commands::create_shop::{
    CreateShopCommand, CreateShopError, CreateShopUseCase,
};
use shop_service::use_cases::commands::update_shop::{
    UpdateShopCommand, UpdateShopError, UpdateShopUseCase,
};
use shop_service::use_cases::queries::check_user_partner_shop::{
    CheckUserPartnerShopError, CheckUserPartnerShopRequest, CheckUserPartnerShopResult,
    CheckUserPartnerShopUseCase,
};
use shop_service::use_cases::queries::get_shop::GetShopUseCase;
use shop_service::use_cases::queries::list_user_partner_shops::{
    ListUserPartnerShopsError, ListUserPartnerShopsRequest, ListUserPartnerShopsResult,
    ListUserPartnerShopsUseCase,
};
use shop_service::use_cases::queries::search_shops::{
    SearchShopsError, SearchShopsRequest, SearchShopsResult, SearchShopsUseCase,
};
use std::sync::Arc;
use user_service::use_cases::queries::check_user_admin::{
    CheckUserAdminError, CheckUserAdminRequest, CheckUserAdminResult, CheckUserAdminUseCase,
};

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
    pub(crate) check_user_admin: Arc<dyn CheckUserAdminUseCase>,
    pub(crate) check_user_partner_shop: Arc<dyn CheckUserPartnerShopUseCase>,
    pub(crate) list_user_partner_shops: Arc<dyn ListUserPartnerShopsUseCase>,
    pub(crate) authenticator: Arc<dyn TokenAuthenticator>,
}

impl ShopsState {
    pub fn new(
        get_shop: Arc<dyn GetShopUseCase>,
        authenticator: Arc<dyn TokenAuthenticator>,
    ) -> Self {
        Self::with_all(
            get_shop,
            Arc::new(UnavailableSearchShops),
            Arc::new(UnavailableCreateShop),
            Arc::new(UnavailableUpdateShop),
            Arc::new(UnavailableCheckUserAdmin),
            Arc::new(UnavailableCheckUserPartnerShop),
            Arc::new(UnavailableListUserPartnerShops),
            authenticator,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn with_all(
        get_shop: Arc<dyn GetShopUseCase>,
        search_shops: Arc<dyn SearchShopsUseCase>,
        create_shop: Arc<dyn CreateShopUseCase>,
        update_shop: Arc<dyn UpdateShopUseCase>,
        check_user_admin: Arc<dyn CheckUserAdminUseCase>,
        check_user_partner_shop: Arc<dyn CheckUserPartnerShopUseCase>,
        list_user_partner_shops: Arc<dyn ListUserPartnerShopsUseCase>,
        authenticator: Arc<dyn TokenAuthenticator>,
    ) -> Self {
        Self {
            get_shop,
            search_shops,
            create_shop,
            update_shop,
            check_user_admin,
            check_user_partner_shop,
            list_user_partner_shops,
            authenticator,
        }
    }
}

struct UnavailableSearchShops;
struct UnavailableCreateShop;
struct UnavailableUpdateShop;
struct UnavailableCheckUserAdmin;
struct UnavailableCheckUserPartnerShop;
struct UnavailableListUserPartnerShops;

#[async_trait::async_trait]
impl SearchShopsUseCase for UnavailableSearchShops {
    async fn execute(
        &self,
        _context: &OperationContext,
        _request: SearchShopsRequest,
    ) -> Result<SearchShopsResult, SearchShopsError> {
        Err(SearchShopsError::Internal {
            source: static_error("search shops use case not configured"),
        })
    }
}

#[async_trait::async_trait]
impl CreateShopUseCase for UnavailableCreateShop {
    async fn execute(
        &self,
        _context: &OperationContext,
        _command: CreateShopCommand,
    ) -> Result<shop_service::use_cases::commands::create_shop::CreateShopResult, CreateShopError>
    {
        Err(CreateShopError::Internal {
            source: static_error("create shop use case not configured"),
        })
    }
}

#[async_trait::async_trait]
impl UpdateShopUseCase for UnavailableUpdateShop {
    async fn execute(
        &self,
        _context: &OperationContext,
        _command: UpdateShopCommand,
    ) -> Result<shop_service::use_cases::commands::update_shop::UpdateShopResult, UpdateShopError>
    {
        Err(UpdateShopError::Internal {
            source: static_error("update shop use case not configured"),
        })
    }
}

#[async_trait::async_trait]
impl CheckUserAdminUseCase for UnavailableCheckUserAdmin {
    async fn execute(
        &self,
        _context: &OperationContext,
        _request: CheckUserAdminRequest,
    ) -> Result<CheckUserAdminResult, CheckUserAdminError> {
        Err(CheckUserAdminError::Internal {
            source: static_error("check user admin use case not configured"),
        })
    }
}

#[async_trait::async_trait]
impl CheckUserPartnerShopUseCase for UnavailableCheckUserPartnerShop {
    async fn execute(
        &self,
        _context: &OperationContext,
        _request: CheckUserPartnerShopRequest,
    ) -> Result<CheckUserPartnerShopResult, CheckUserPartnerShopError> {
        Err(CheckUserPartnerShopError::Internal {
            source: static_error("check user partner shop use case not configured"),
        })
    }
}

#[async_trait::async_trait]
impl ListUserPartnerShopsUseCase for UnavailableListUserPartnerShops {
    async fn execute(
        &self,
        _context: &OperationContext,
        _request: ListUserPartnerShopsRequest,
    ) -> Result<ListUserPartnerShopsResult, ListUserPartnerShopsError> {
        Err(ListUserPartnerShopsError::Internal {
            source: static_error("list user partner shops use case not configured"),
        })
    }
}
