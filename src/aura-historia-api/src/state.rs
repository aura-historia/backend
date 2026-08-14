use crate::auth::TokenAuthenticator;
use async_trait::async_trait;
use billing_service::use_cases::{
    CreateBillingCheckoutSessionUseCase, CreateBillingManagementSessionUseCase,
    CreateBillingPortalSessionUseCase,
};
use oauth_service::use_cases::{
    AuthorizeUseCase, CreateOAuthClientUseCase, DeleteOAuthClientUseCase, GetOAuthClientUseCase,
    IntrospectTokenUseCase, ListOAuthClientsUseCase, RevokeTokenUseCase,
    TokenByAuthorizationCodeUseCase, TokenByThirdPartyCodeUseCase, UpdateOAuthClientUseCase,
};
use product_service::use_cases::{
    CreateProductUseCase, DeleteProductUseCase, GetProductEventsUseCase, GetProductUseCase,
    GetSimilarProductsUseCase, IngestWoocommerceProductUseCase, SearchProductsUseCase,
    UpdateProductUseCase, UpsertProductUseCase,
};
use search_filter_service::use_cases::{
    CreateSearchFilterUseCase, DeleteOwnedSearchFilterUseCase, GetOwnedSearchFilterUseCase,
    ListOwnedSearchFiltersUseCase, ListSearchFilterMatchesUseCase, UpdateOwnedSearchFilterUseCase,
    UpdateSearchFilterMatchFeedbackUseCase,
};
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
use user_service::use_cases::commands::upsert_newsletter_subscription::UpsertNewsletterSubscriptionUseCase;
use user_service::use_cases::queries::admin_get_user::AdminGetUserUseCase;
use user_service::use_cases::queries::get_access_token::GetAccessTokenUseCase;
use user_service::use_cases::queries::get_own_user::GetOwnUserUseCase;
use user_service::use_cases::queries::list_access_tokens::ListAccessTokensUseCase;
use user_service::use_cases::queries::search_users::SearchUsersUseCase;
use watchlist_service::use_cases::{
    ListWatchlistUseCase, UnwatchProductUseCase, UpdateWatchlistProductUseCase, WatchProductUseCase,
};

#[async_trait]
pub(crate) trait ReadinessCheck: Send + Sync {
    async fn check(&self) -> Result<(), ()>;
}

#[derive(Clone, Copy)]
struct AlwaysReady;

#[async_trait]
impl ReadinessCheck for AlwaysReady {
    async fn check(&self) -> Result<(), ()> {
        Ok(())
    }
}

#[derive(Clone)]
pub struct AppState {
    pub(crate) readiness: Arc<dyn ReadinessCheck>,
    pub(crate) shops: ShopsState,
    pub(crate) products: Option<ProductsState>,
    pub(crate) partner_products: Option<PartnerProductsState>,
    pub(crate) users: Option<UsersState>,
    pub(crate) watchlist: Option<WatchlistState>,
    pub(crate) partner_applications: Option<PartnerApplicationsState>,
    pub(crate) oauth: Option<OAuthState>,
    pub(crate) search_filters: Option<SearchFiltersState>,
    pub(crate) billing: Option<BillingState>,
    pub(crate) newsletter: Option<NewsletterState>,
    pub(crate) webhooks: Option<WebhooksState>,
}

impl AppState {
    pub fn new(
        shops: ShopsState,
        users: UsersState,
        watchlist: WatchlistState,
        partner_applications: PartnerApplicationsState,
    ) -> Self {
        Self {
            readiness: Arc::new(AlwaysReady),
            shops,
            products: None,
            partner_products: None,
            users: Some(users),
            watchlist: Some(watchlist),
            partner_applications: Some(partner_applications),
            oauth: None,
            search_filters: None,
            billing: None,
            newsletter: None,
            webhooks: None,
        }
    }

    pub fn with_shops_only(shops: ShopsState) -> Self {
        Self {
            readiness: Arc::new(AlwaysReady),
            shops,
            products: None,
            partner_products: None,
            users: None,
            watchlist: None,
            partner_applications: None,
            oauth: None,
            search_filters: None,
            billing: None,
            newsletter: None,
            webhooks: None,
        }
    }

    pub(crate) fn with_readiness(mut self, readiness: Arc<dyn ReadinessCheck>) -> Self {
        self.readiness = readiness;
        self
    }

    pub fn with_products(mut self, products: ProductsState) -> Self {
        self.products = Some(products);
        self
    }

    pub fn with_partner_products(mut self, partner_products: PartnerProductsState) -> Self {
        self.partner_products = Some(partner_products);
        self
    }

    pub fn with_oauth(mut self, oauth: OAuthState) -> Self {
        self.oauth = Some(oauth);
        self
    }

    pub fn with_search_filters(mut self, search_filters: SearchFiltersState) -> Self {
        self.search_filters = Some(search_filters);
        self
    }

    pub fn with_billing(mut self, billing: BillingState) -> Self {
        self.billing = Some(billing);
        self
    }

    pub fn with_newsletter(mut self, newsletter: NewsletterState) -> Self {
        self.newsletter = Some(newsletter);
        self
    }

    pub fn with_webhooks(mut self, webhooks: WebhooksState) -> Self {
        self.webhooks = Some(webhooks);
        self
    }
}

#[derive(Clone)]
pub struct WebhooksState {
    pub(crate) ingest: Arc<dyn IngestWoocommerceProductUseCase>,
    pub(crate) authenticator: Arc<dyn TokenAuthenticator>,
}

impl WebhooksState {
    pub fn new(
        ingest: Arc<dyn IngestWoocommerceProductUseCase>,
        authenticator: Arc<dyn TokenAuthenticator>,
    ) -> Self {
        Self {
            ingest,
            authenticator,
        }
    }
}

#[derive(Clone)]
pub struct BillingState {
    pub(crate) checkout: Arc<dyn CreateBillingCheckoutSessionUseCase>,
    pub(crate) portal: Arc<dyn CreateBillingPortalSessionUseCase>,
    pub(crate) manage: Arc<dyn CreateBillingManagementSessionUseCase>,
    pub(crate) authenticator: Arc<dyn TokenAuthenticator>,
}

impl BillingState {
    pub fn new(
        checkout: Arc<dyn CreateBillingCheckoutSessionUseCase>,
        portal: Arc<dyn CreateBillingPortalSessionUseCase>,
        manage: Arc<dyn CreateBillingManagementSessionUseCase>,
        authenticator: Arc<dyn TokenAuthenticator>,
    ) -> Self {
        Self {
            checkout,
            portal,
            manage,
            authenticator,
        }
    }
}

#[derive(Clone)]
pub struct NewsletterState {
    pub(crate) upsert_subscription: Arc<dyn UpsertNewsletterSubscriptionUseCase>,
    pub(crate) authenticator: Arc<dyn TokenAuthenticator>,
}

impl NewsletterState {
    pub fn new(
        upsert_subscription: Arc<dyn UpsertNewsletterSubscriptionUseCase>,
        authenticator: Arc<dyn TokenAuthenticator>,
    ) -> Self {
        Self {
            upsert_subscription,
            authenticator,
        }
    }
}

#[derive(Clone)]
pub struct SearchFiltersState {
    pub(crate) list_owned_search_filters: Arc<dyn ListOwnedSearchFiltersUseCase>,
    pub(crate) create_search_filter: Arc<dyn CreateSearchFilterUseCase>,
    pub(crate) get_owned_search_filter: Arc<dyn GetOwnedSearchFilterUseCase>,
    pub(crate) update_owned_search_filter: Arc<dyn UpdateOwnedSearchFilterUseCase>,
    pub(crate) delete_owned_search_filter: Arc<dyn DeleteOwnedSearchFilterUseCase>,
    pub(crate) list_search_filter_matches: Arc<dyn ListSearchFilterMatchesUseCase>,
    pub(crate) update_search_filter_match_feedback: Arc<dyn UpdateSearchFilterMatchFeedbackUseCase>,
    pub(crate) authenticator: Arc<dyn TokenAuthenticator>,
}

impl SearchFiltersState {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        list_owned_search_filters: Arc<dyn ListOwnedSearchFiltersUseCase>,
        create_search_filter: Arc<dyn CreateSearchFilterUseCase>,
        get_owned_search_filter: Arc<dyn GetOwnedSearchFilterUseCase>,
        update_owned_search_filter: Arc<dyn UpdateOwnedSearchFilterUseCase>,
        delete_owned_search_filter: Arc<dyn DeleteOwnedSearchFilterUseCase>,
        list_search_filter_matches: Arc<dyn ListSearchFilterMatchesUseCase>,
        update_search_filter_match_feedback: Arc<dyn UpdateSearchFilterMatchFeedbackUseCase>,
        authenticator: Arc<dyn TokenAuthenticator>,
    ) -> Self {
        Self {
            list_owned_search_filters,
            create_search_filter,
            get_owned_search_filter,
            update_owned_search_filter,
            delete_owned_search_filter,
            list_search_filter_matches,
            update_search_filter_match_feedback,
            authenticator,
        }
    }
}

#[derive(Clone)]
pub struct OAuthState {
    pub(crate) create_client: Arc<dyn CreateOAuthClientUseCase>,
    pub(crate) list_clients: Arc<dyn ListOAuthClientsUseCase>,
    pub(crate) get_client: Arc<dyn GetOAuthClientUseCase>,
    pub(crate) update_client: Arc<dyn UpdateOAuthClientUseCase>,
    pub(crate) delete_client: Arc<dyn DeleteOAuthClientUseCase>,
    pub(crate) authorize: Arc<dyn AuthorizeUseCase>,
    pub(crate) token_by_authorization_code: Arc<dyn TokenByAuthorizationCodeUseCase>,
    pub(crate) token_by_third_party_code: Arc<dyn TokenByThirdPartyCodeUseCase>,
    pub(crate) revoke: Arc<dyn RevokeTokenUseCase>,
    pub(crate) introspect: Arc<dyn IntrospectTokenUseCase>,
    pub(crate) authenticator: Arc<dyn TokenAuthenticator>,
}

impl OAuthState {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        create_client: Arc<dyn CreateOAuthClientUseCase>,
        list_clients: Arc<dyn ListOAuthClientsUseCase>,
        get_client: Arc<dyn GetOAuthClientUseCase>,
        update_client: Arc<dyn UpdateOAuthClientUseCase>,
        delete_client: Arc<dyn DeleteOAuthClientUseCase>,
        authorize: Arc<dyn AuthorizeUseCase>,
        token_by_authorization_code: Arc<dyn TokenByAuthorizationCodeUseCase>,
        token_by_third_party_code: Arc<dyn TokenByThirdPartyCodeUseCase>,
        revoke: Arc<dyn RevokeTokenUseCase>,
        introspect: Arc<dyn IntrospectTokenUseCase>,
        authenticator: Arc<dyn TokenAuthenticator>,
    ) -> Self {
        Self {
            create_client,
            list_clients,
            get_client,
            update_client,
            delete_client,
            authorize,
            token_by_authorization_code,
            token_by_third_party_code,
            revoke,
            introspect,
            authenticator,
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
pub struct ProductsState {
    pub(crate) get_product: Arc<dyn GetProductUseCase>,
    pub(crate) get_product_events: Option<Arc<dyn GetProductEventsUseCase>>,
    pub(crate) get_similar_products: Arc<dyn GetSimilarProductsUseCase>,
    pub(crate) search_products: Arc<dyn SearchProductsUseCase>,
    pub(crate) authenticator: Arc<dyn TokenAuthenticator>,
}

impl ProductsState {
    pub fn new(
        get_product: Arc<dyn GetProductUseCase>,
        get_similar_products: Arc<dyn GetSimilarProductsUseCase>,
        search_products: Arc<dyn SearchProductsUseCase>,
        authenticator: Arc<dyn TokenAuthenticator>,
    ) -> Self {
        Self {
            get_product,
            get_product_events: None,
            get_similar_products,
            search_products,
            authenticator,
        }
    }

    pub fn with_product_events(
        mut self,
        get_product_events: Arc<dyn GetProductEventsUseCase>,
    ) -> Self {
        self.get_product_events = Some(get_product_events);
        self
    }
}

#[derive(Clone)]
pub struct PartnerProductsState {
    pub(crate) create: Arc<dyn CreateProductUseCase>,
    pub(crate) update: Arc<dyn UpdateProductUseCase>,
    pub(crate) upsert: Arc<dyn UpsertProductUseCase>,
    pub(crate) delete: Arc<dyn DeleteProductUseCase>,
    pub(crate) authenticator: Arc<dyn TokenAuthenticator>,
}

impl PartnerProductsState {
    pub fn new(
        create: Arc<dyn CreateProductUseCase>,
        update: Arc<dyn UpdateProductUseCase>,
        upsert: Arc<dyn UpsertProductUseCase>,
        delete: Arc<dyn DeleteProductUseCase>,
        authenticator: Arc<dyn TokenAuthenticator>,
    ) -> Self {
        Self {
            create,
            update,
            upsert,
            delete,
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
