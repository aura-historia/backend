use crate::auth::AuthError;
use axum::Json;
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use billing_service::use_cases::{
    CreateBillingCheckoutSessionError, CreateBillingManagementSessionError,
    CreateBillingPortalSessionError,
};
use oauth_service::error::OAuthServiceError;
use product_service::use_cases::{
    CreateProductError, DeleteProductError, GetProductError, GetProductEventsError,
    GetSimilarProductsError, SearchProductsError, UpdateProductError, UpsertProductError,
};
use search_filter_service::use_cases::{
    CreateSearchFilterError, DeleteOwnedSearchFilterError, GetOwnedSearchFilterError,
    ListOwnedSearchFiltersError, ListSearchFilterMatchesError, UpdateOwnedSearchFilterError,
    UpdateSearchFilterMatchFeedbackError,
};
use serde::Serialize;
use shop_partner_service::use_cases::{
    AdminDecidePartnerShopApplicationError, AdminGetPartnerShopApplicationError,
    AdminListPartnerShopApplicationsError, AdminUpdatePartnerShopApplicationError,
    CreatePartnerShopApplicationError, GetPartnerShopApplicationError,
    ListPartnerShopApplicationsError, WithdrawPartnerShopApplicationError,
};
use shop_service::use_cases::commands::create_shop::CreateShopError;
use shop_service::use_cases::commands::update_shop::UpdateShopError;
use shop_service::use_cases::queries::check_user_partner_shop::CheckUserPartnerShopError;
use shop_service::use_cases::queries::get_shop::GetShopError;
use shop_service::use_cases::queries::list_user_partner_shops::ListUserPartnerShopsError;
use shop_service::use_cases::queries::search_shops::SearchShopsError;
use std::error::Error;
use std::fmt::{Display, Formatter};
use user_service::use_cases::commands::change_user_role::ChangeUserRoleError;
use user_service::use_cases::commands::change_user_tier::ChangeUserTierError;
use user_service::use_cases::commands::create_access_token::CreateAccessTokenError;
use user_service::use_cases::commands::delete_access_token::DeleteAccessTokenError;
use user_service::use_cases::commands::delete_user::DeleteUserError;
use user_service::use_cases::commands::update_access_token::UpdateAccessTokenError;
use user_service::use_cases::commands::update_user_profile::UpdateUserProfileError;
use user_service::use_cases::commands::upsert_newsletter_subscription::UpsertNewsletterSubscriptionError;
use user_service::use_cases::queries::admin_get_user::AdminGetUserError;
use user_service::use_cases::queries::check_user_admin::CheckUserAdminError;
use user_service::use_cases::queries::get_access_token::GetAccessTokenError;
use user_service::use_cases::queries::get_own_user::GetOwnUserError;
use user_service::use_cases::queries::list_access_tokens::ListAccessTokensError;
use user_service::use_cases::queries::search_users::SearchUsersError;
use watchlist_service::use_cases::{
    ListWatchlistError, UnwatchProductError, UpdateWatchlistProductError, WatchProductError,
};

#[derive(Debug, Serialize)]
pub(crate) struct ApiError {
    status: u16,
    title: &'static str,
    error: ApiErrorCode,
    #[serde(skip_serializing_if = "Option::is_none")]
    source: Option<ApiErrorSource>,
    #[serde(skip_serializing_if = "Option::is_none")]
    detail: Option<String>,
    #[serde(skip)]
    cause: Option<Box<dyn Error + Send + Sync>>,
}

#[derive(Debug, Serialize, PartialEq, Eq, Clone, Copy)]
#[serde(transparent)]
pub(crate) struct ApiErrorCode(&'static str);

pub(crate) const AUTH_INTERNAL_ERROR: ApiErrorCode = ApiErrorCode("AUTH_INTERNAL_ERROR");
pub(crate) const AUTH_TEMPORARILY_UNAVAILABLE: ApiErrorCode =
    ApiErrorCode("AUTH_TEMPORARILY_UNAVAILABLE");
pub(crate) const ACCESS_TOKEN_INTERNAL_ERROR: ApiErrorCode =
    ApiErrorCode("ACCESS_TOKEN_INTERNAL_ERROR");
pub(crate) const ACCESS_TOKEN_NOT_FOUND: ApiErrorCode = ApiErrorCode("ACCESS_TOKEN_NOT_FOUND");
pub(crate) const ACCESS_TOKEN_TEMPORARILY_UNAVAILABLE: ApiErrorCode =
    ApiErrorCode("ACCESS_TOKEN_TEMPORARILY_UNAVAILABLE");
pub(crate) const INVALID_CREDENTIALS: ApiErrorCode = ApiErrorCode("INVALID_CREDENTIALS");
pub(crate) const BAD_BODY_VALUE: ApiErrorCode = ApiErrorCode("BAD_BODY_VALUE");
pub(crate) const BILLING_INTERNAL_ERROR: ApiErrorCode = ApiErrorCode("BILLING_INTERNAL_ERROR");
pub(crate) const BILLING_PROVIDER_FAILURE: ApiErrorCode = ApiErrorCode("BILLING_PROVIDER_FAILURE");
pub(crate) const BILLING_TEMPORARILY_UNAVAILABLE: ApiErrorCode =
    ApiErrorCode("BILLING_TEMPORARILY_UNAVAILABLE");
pub(crate) const STRIPE_CUSTOMER_ALREADY_EXISTS: ApiErrorCode =
    ApiErrorCode("STRIPE_CUSTOMER_ALREADY_EXISTS");
pub(crate) const STRIPE_CUSTOMER_ASSOCIATION_CONFLICT: ApiErrorCode =
    ApiErrorCode("STRIPE_CUSTOMER_ASSOCIATION_CONFLICT");
pub(crate) const STRIPE_CUSTOMER_DOES_NOT_EXIST: ApiErrorCode =
    ApiErrorCode("STRIPE_CUSTOMER_DOES_NOT_EXIST");
pub(crate) const BAD_ORDER_VALUE: ApiErrorCode = ApiErrorCode("BAD_ORDER_VALUE");
pub(crate) const BAD_PATH_PARAMETER_VALUE: ApiErrorCode = ApiErrorCode("BAD_PATH_PARAMETER_VALUE");
pub(crate) const BAD_QUERY_PARAMETER_VALUE: ApiErrorCode =
    ApiErrorCode("BAD_QUERY_PARAMETER_VALUE");
pub(crate) const BAD_SORT_VALUE: ApiErrorCode = ApiErrorCode("BAD_SORT_VALUE");
pub(crate) const CONFLICT: ApiErrorCode = ApiErrorCode("CONFLICT");
pub(crate) const FORBIDDEN: ApiErrorCode = ApiErrorCode("FORBIDDEN");
pub(crate) const INVALID_UUID: ApiErrorCode = ApiErrorCode("INVALID_UUID");
pub(crate) const SHOP_EXISTS_ALREADY: ApiErrorCode = ApiErrorCode("SHOP_EXISTS_ALREADY");
pub(crate) const SHOP_INTERNAL_ERROR: ApiErrorCode = ApiErrorCode("SHOP_INTERNAL_ERROR");
pub(crate) const SHOP_NOT_FOUND: ApiErrorCode = ApiErrorCode("SHOP_NOT_FOUND");
pub(crate) const SHOP_TEMPORARILY_UNAVAILABLE: ApiErrorCode =
    ApiErrorCode("SHOP_TEMPORARILY_UNAVAILABLE");
pub(crate) const PRODUCT_INTERNAL_ERROR: ApiErrorCode = ApiErrorCode("PRODUCT_INTERNAL_ERROR");
pub(crate) const PRODUCT_NOT_FOUND: ApiErrorCode = ApiErrorCode("PRODUCT_NOT_FOUND");
pub(crate) const PRODUCT_TEMPORARILY_UNAVAILABLE: ApiErrorCode =
    ApiErrorCode("PRODUCT_TEMPORARILY_UNAVAILABLE");
pub(crate) const SEARCH_FILTER_ALREADY_EXISTS: ApiErrorCode =
    ApiErrorCode("SEARCH_FILTER_ALREADY_EXISTS");
pub(crate) const SEARCH_FILTER_INTERNAL_ERROR: ApiErrorCode =
    ApiErrorCode("SEARCH_FILTER_INTERNAL_ERROR");
pub(crate) const SEARCH_FILTER_INVALID_PATCH: ApiErrorCode =
    ApiErrorCode("SEARCH_FILTER_INVALID_PATCH");
pub(crate) const SEARCH_FILTER_MATCH_NOT_FOUND: ApiErrorCode =
    ApiErrorCode("SEARCH_FILTER_MATCH_NOT_FOUND");
pub(crate) const SEARCH_FILTER_NOT_FOUND: ApiErrorCode = ApiErrorCode("SEARCH_FILTER_NOT_FOUND");
pub(crate) const SEARCH_FILTER_QUOTA_EXCEEDED: ApiErrorCode =
    ApiErrorCode("SEARCH_FILTER_QUOTA_EXCEEDED");
pub(crate) const SEARCH_FILTER_RESTRICTED_FEATURE: ApiErrorCode =
    ApiErrorCode("SEARCH_FILTER_RESTRICTED_FEATURE");
pub(crate) const SEARCH_FILTER_TEMPORARILY_UNAVAILABLE: ApiErrorCode =
    ApiErrorCode("SEARCH_FILTER_TEMPORARILY_UNAVAILABLE");
pub(crate) const INVALID_EMAIL: ApiErrorCode = ApiErrorCode("INVALID_EMAIL");
pub(crate) const NEWSLETTER_INTERNAL_ERROR: ApiErrorCode =
    ApiErrorCode("NEWSLETTER_INTERNAL_ERROR");
pub(crate) const NEWSLETTER_TEMPORARILY_UNAVAILABLE: ApiErrorCode =
    ApiErrorCode("NEWSLETTER_TEMPORARILY_UNAVAILABLE");
pub(crate) const USER_INTERNAL_ERROR: ApiErrorCode = ApiErrorCode("USER_INTERNAL_ERROR");
pub(crate) const USER_NOT_FOUND: ApiErrorCode = ApiErrorCode("USER_NOT_FOUND");
pub(crate) const USER_TEMPORARILY_UNAVAILABLE: ApiErrorCode =
    ApiErrorCode("USER_TEMPORARILY_UNAVAILABLE");
pub(crate) const WATCHLIST_ENTRY_NOT_FOUND: ApiErrorCode =
    ApiErrorCode("WATCHLIST_ENTRY_NOT_FOUND");
pub(crate) const WATCHLIST_QUOTA_EXCEEDED: ApiErrorCode = ApiErrorCode("WATCHLIST_QUOTA_EXCEEDED");
pub(crate) const WATCHLIST_INTERNAL_ERROR: ApiErrorCode = ApiErrorCode("WATCHLIST_INTERNAL_ERROR");
pub(crate) const WATCHLIST_TEMPORARILY_UNAVAILABLE: ApiErrorCode =
    ApiErrorCode("WATCHLIST_TEMPORARILY_UNAVAILABLE");
pub(crate) const PARTNER_SHOP_APPLICATION_NOT_FOUND: ApiErrorCode =
    ApiErrorCode("PARTNER_SHOP_APPLICATION_NOT_FOUND");
pub(crate) const PARTNER_SHOP_APPLICATION_INTERNAL_ERROR: ApiErrorCode =
    ApiErrorCode("PARTNER_SHOP_APPLICATION_INTERNAL_ERROR");
pub(crate) const PARTNER_SHOP_APPLICATION_TEMPORARILY_UNAVAILABLE: ApiErrorCode =
    ApiErrorCode("PARTNER_SHOP_APPLICATION_TEMPORARILY_UNAVAILABLE");
pub(crate) const OAUTH_AUTHORIZATION_CODE_EXPIRED: ApiErrorCode =
    ApiErrorCode("OAUTH_AUTHORIZATION_CODE_EXPIRED");
pub(crate) const OAUTH_AUTHORIZATION_CODE_NOT_FOUND: ApiErrorCode =
    ApiErrorCode("OAUTH_AUTHORIZATION_CODE_NOT_FOUND");
pub(crate) const OAUTH_CLIENT_NOT_FOUND: ApiErrorCode = ApiErrorCode("OAUTH_CLIENT_NOT_FOUND");
pub(crate) const OAUTH_INTERNAL_ERROR: ApiErrorCode = ApiErrorCode("OAUTH_INTERNAL_ERROR");
pub(crate) const OAUTH_INVALID_CLIENT_METADATA: ApiErrorCode =
    ApiErrorCode("OAUTH_INVALID_CLIENT_METADATA");
pub(crate) const OAUTH_INVALID_CLIENT_SECRET: ApiErrorCode =
    ApiErrorCode("OAUTH_INVALID_CLIENT_SECRET");
pub(crate) const OAUTH_INVALID_CODE_VERIFIER: ApiErrorCode =
    ApiErrorCode("OAUTH_INVALID_CODE_VERIFIER");
pub(crate) const OAUTH_INVALID_REDIRECT_URI: ApiErrorCode =
    ApiErrorCode("OAUTH_INVALID_REDIRECT_URI");
pub(crate) const OAUTH_INVALID_SCOPE: ApiErrorCode = ApiErrorCode("OAUTH_INVALID_SCOPE");
pub(crate) const OAUTH_TEMPORARILY_UNAVAILABLE: ApiErrorCode =
    ApiErrorCode("OAUTH_TEMPORARILY_UNAVAILABLE");
pub(crate) const OAUTH_THIRD_PARTY_EXCHANGE_CODE_NOT_FOUND: ApiErrorCode =
    ApiErrorCode("OAUTH_THIRD_PARTY_EXCHANGE_CODE_NOT_FOUND");

impl Display for ApiErrorCode {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[derive(Debug, Serialize, PartialEq, Eq, Clone, Copy)]
pub(crate) struct ApiErrorSource {
    field: &'static str,
    #[serde(rename = "type")]
    source_type: ApiErrorSourceType,
}

#[derive(Debug, Serialize, PartialEq, Eq, Clone, Copy)]
#[serde(rename_all = "UPPERCASE")]
enum ApiErrorSourceType {
    Header,
    Path,
    Query,
}

impl ApiError {
    pub(crate) fn code(&self) -> ApiErrorCode {
        self.error
    }

    pub(crate) fn new(status: StatusCode, title: &'static str, error: ApiErrorCode) -> Self {
        Self {
            status: status.as_u16(),
            title,
            error,
            source: None,
            detail: None,
            cause: None,
        }
    }

    pub(crate) fn bad_request(error: ApiErrorCode) -> Self {
        Self::new(StatusCode::BAD_REQUEST, "Bad Request", error)
    }

    pub(crate) fn unauthorized(error: ApiErrorCode) -> Self {
        Self::new(StatusCode::UNAUTHORIZED, "Unauthorized", error)
    }

    pub(crate) fn forbidden(error: ApiErrorCode) -> Self {
        Self::new(StatusCode::FORBIDDEN, "Forbidden", error)
    }

    pub(crate) fn not_found(error: ApiErrorCode) -> Self {
        Self::new(StatusCode::NOT_FOUND, "Not Found", error)
    }

    pub(crate) fn conflict(error: ApiErrorCode) -> Self {
        Self::new(StatusCode::CONFLICT, "Conflict", error)
    }

    pub(crate) fn unprocessable_content(error: ApiErrorCode) -> Self {
        Self::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            "Unprocessable Content",
            error,
        )
    }

    pub(crate) fn internal_server_error(error: ApiErrorCode) -> Self {
        Self::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "Internal Server Error",
            error,
        )
    }

    pub(crate) fn service_unavailable(error: ApiErrorCode) -> Self {
        Self::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "Service Unavailable",
            error,
        )
    }

    pub(crate) fn with_header_field(mut self, field: &'static str) -> Self {
        self.source = Some(ApiErrorSource {
            field,
            source_type: ApiErrorSourceType::Header,
        });
        self
    }

    pub(crate) fn with_query_field(mut self, field: &'static str) -> Self {
        self.source = Some(ApiErrorSource {
            field,
            source_type: ApiErrorSourceType::Query,
        });
        self
    }

    pub(crate) fn with_path_field(mut self, field: &'static str) -> Self {
        self.source = Some(ApiErrorSource {
            field,
            source_type: ApiErrorSourceType::Path,
        });
        self
    }

    pub(crate) fn with_detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = Some(detail.into());
        self
    }

    fn status_code(&self) -> StatusCode {
        match StatusCode::from_u16(self.status) {
            Ok(status) => status,
            Err(_) => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }
}

impl Display for ApiError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "HTTP {} - {}", self.status, self.error)?;
        if let Some(detail) = &self.detail {
            write!(f, ": {detail}")?;
        }
        if let Some(cause) = &self.cause {
            write!(f, ": {cause}")?;
        }
        Ok(())
    }
}

impl Error for ApiError {}

impl From<AuthError> for ApiError {
    fn from(error: AuthError) -> Self {
        match error {
            AuthError::TemporarilyUnavailable => {
                ApiError::service_unavailable(AUTH_TEMPORARILY_UNAVAILABLE)
                    .with_detail("Authentication is temporarily unavailable.")
            }
            AuthError::Internal(_) => ApiError::internal_server_error(AUTH_INTERNAL_ERROR)
                .with_detail("Authentication failed internally."),
            AuthError::MissingCredentials
            | AuthError::InvalidAuthorizationHeader
            | AuthError::MalformedCredentials
            | AuthError::InvalidCredentials
            | AuthError::MissingClaim(_)
            | AuthError::InvalidClaimType(_)
            | AuthError::JwksKeyNotFound
            | AuthError::JwksFetch(_) => ApiError::unauthorized(INVALID_CREDENTIALS)
                .with_header_field("Authorization")
                .with_detail("Bearer token is invalid."),
        }
    }
}

impl From<ListOwnedSearchFiltersError> for ApiError {
    fn from(error: ListOwnedSearchFiltersError) -> Self {
        match error {
            ListOwnedSearchFiltersError::AuthenticatedActorRequired => {
                ApiError::unauthorized(INVALID_CREDENTIALS)
                    .with_header_field("Authorization")
                    .with_detail("Bearer token is required.")
            }
            ListOwnedSearchFiltersError::ActorMayNotManageSearchFilter => {
                ApiError::forbidden(FORBIDDEN).with_detail("Operation is not permitted.")
            }
            ListOwnedSearchFiltersError::SearchFilterListReadFailed { .. } => {
                ApiError::service_unavailable(SEARCH_FILTER_TEMPORARILY_UNAVAILABLE)
                    .with_detail("Search filters are temporarily unavailable.")
            }
        }
    }
}

impl From<GetOwnedSearchFilterError> for ApiError {
    fn from(error: GetOwnedSearchFilterError) -> Self {
        match error {
            GetOwnedSearchFilterError::AuthenticatedActorRequired => {
                ApiError::unauthorized(INVALID_CREDENTIALS)
                    .with_header_field("Authorization")
                    .with_detail("Bearer token is required.")
            }
            GetOwnedSearchFilterError::ActorMayNotManageSearchFilter => {
                ApiError::forbidden(FORBIDDEN).with_detail("Operation is not permitted.")
            }
            GetOwnedSearchFilterError::SearchFilterNotFound => {
                ApiError::not_found(SEARCH_FILTER_NOT_FOUND)
                    .with_detail("Search filter was not found.")
            }
            GetOwnedSearchFilterError::SearchFilterReadFailed { .. } => {
                ApiError::service_unavailable(SEARCH_FILTER_TEMPORARILY_UNAVAILABLE)
                    .with_detail("Search filter is temporarily unavailable.")
            }
        }
    }
}

impl From<CreateSearchFilterError> for ApiError {
    fn from(error: CreateSearchFilterError) -> Self {
        match error {
            CreateSearchFilterError::AuthenticatedActorRequired => {
                ApiError::unauthorized(INVALID_CREDENTIALS)
                    .with_header_field("Authorization")
                    .with_detail("Bearer token is required.")
            }
            CreateSearchFilterError::ActorMayNotManageSearchFilter => {
                ApiError::forbidden(FORBIDDEN).with_detail("Operation is not permitted.")
            }
            CreateSearchFilterError::SearchFilterAlreadyExists => {
                ApiError::conflict(SEARCH_FILTER_ALREADY_EXISTS)
                    .with_detail("Search filter already exists.")
            }
            CreateSearchFilterError::UserNotFound => ApiError::not_found(USER_NOT_FOUND)
                .with_detail("User was not found."),
            CreateSearchFilterError::SearchFilterQuotaExceeded {
                active_count,
                quota,
            } => ApiError::unprocessable_content(SEARCH_FILTER_QUOTA_EXCEEDED).with_detail(
                format!(
                    "Exceeded the maximum amount of search filters. There are already {active_count}/{quota} active search filters occupied."
                ),
            ),
            CreateSearchFilterError::SearchFilterFeatureRestricted { feature } => {
                ApiError::unprocessable_content(SEARCH_FILTER_RESTRICTED_FEATURE).with_detail(
                    format!(
                        "Search filter contains forbidden search field '{feature}' which requires a higher user tier."
                    ),
                )
            }
            CreateSearchFilterError::PersistedSearchFilterStateInvalid { .. } => {
                ApiError::internal_server_error(SEARCH_FILTER_INTERNAL_ERROR)
                    .with_detail("Search filter state is invalid.")
            }
            CreateSearchFilterError::EmbeddingGenerationFailed { .. }
            | CreateSearchFilterError::UserTierEntitlementsLockFailed { .. }
            | CreateSearchFilterError::SearchFilterQuotaReadFailed { .. }
            | CreateSearchFilterError::SearchFilterInsertFailed { .. }
            | CreateSearchFilterError::BeginTransactionFailed
            | CreateSearchFilterError::CommitTransactionFailed => {
                ApiError::service_unavailable(SEARCH_FILTER_TEMPORARILY_UNAVAILABLE)
                    .with_detail("Search filter could not be created right now.")
            }
        }
    }
}

impl From<UpdateOwnedSearchFilterError> for ApiError {
    fn from(error: UpdateOwnedSearchFilterError) -> Self {
        match error {
            UpdateOwnedSearchFilterError::AuthenticatedActorRequired => {
                ApiError::unauthorized(INVALID_CREDENTIALS)
                    .with_header_field("Authorization")
                    .with_detail("Bearer token is required.")
            }
            UpdateOwnedSearchFilterError::ActorMayNotManageSearchFilter => {
                ApiError::forbidden(FORBIDDEN).with_detail("Operation is not permitted.")
            }
            UpdateOwnedSearchFilterError::SearchFilterNotFound => {
                ApiError::not_found(SEARCH_FILTER_NOT_FOUND)
                    .with_detail("Search filter was not found.")
            }
            UpdateOwnedSearchFilterError::UserNotFound => ApiError::not_found(USER_NOT_FOUND)
                .with_detail("User was not found."),
            UpdateOwnedSearchFilterError::SearchFilterQuotaExceeded {
                active_count,
                quota,
            } => ApiError::unprocessable_content(SEARCH_FILTER_QUOTA_EXCEEDED).with_detail(
                format!(
                    "Exceeded the maximum amount of search filters. There are already {active_count}/{quota} active search filters occupied."
                ),
            ),
            UpdateOwnedSearchFilterError::SearchFilterFeatureRestricted { feature } => {
                ApiError::unprocessable_content(SEARCH_FILTER_RESTRICTED_FEATURE).with_detail(
                    format!(
                        "Search filter contains forbidden search field '{feature}' which requires a higher user tier."
                    ),
                )
            }
            UpdateOwnedSearchFilterError::InvalidSearchFilterPatch => {
                ApiError::bad_request(SEARCH_FILTER_INVALID_PATCH)
                    .with_detail("Search filter patch is invalid.")
            }
            UpdateOwnedSearchFilterError::SearchFilterConcurrencyConflict => {
                ApiError::conflict(CONFLICT).with_detail("Search filter was changed concurrently.")
            }
            UpdateOwnedSearchFilterError::PersistedSearchFilterStateInvalid { .. } => {
                ApiError::internal_server_error(SEARCH_FILTER_INTERNAL_ERROR)
                    .with_detail("Search filter state is invalid.")
            }
            UpdateOwnedSearchFilterError::EmbeddingGenerationFailed { .. }
            | UpdateOwnedSearchFilterError::UserTierEntitlementsLockFailed { .. }
            | UpdateOwnedSearchFilterError::SearchFilterQuotaReadFailed { .. }
            | UpdateOwnedSearchFilterError::SearchFilterLookupFailed { .. }
            | UpdateOwnedSearchFilterError::SearchFilterUpdateFailed { .. }
            | UpdateOwnedSearchFilterError::BeginTransactionFailed
            | UpdateOwnedSearchFilterError::CommitTransactionFailed => {
                ApiError::service_unavailable(SEARCH_FILTER_TEMPORARILY_UNAVAILABLE)
                    .with_detail("Search filter could not be updated right now.")
            }
        }
    }
}

impl From<DeleteOwnedSearchFilterError> for ApiError {
    fn from(error: DeleteOwnedSearchFilterError) -> Self {
        match error {
            DeleteOwnedSearchFilterError::AuthenticatedActorRequired => {
                ApiError::unauthorized(INVALID_CREDENTIALS)
                    .with_header_field("Authorization")
                    .with_detail("Bearer token is required.")
            }
            DeleteOwnedSearchFilterError::ActorMayNotManageSearchFilter => {
                ApiError::forbidden(FORBIDDEN).with_detail("Operation is not permitted.")
            }
            DeleteOwnedSearchFilterError::SearchFilterNotFound => {
                ApiError::not_found(SEARCH_FILTER_NOT_FOUND)
                    .with_detail("Search filter was not found.")
            }
            DeleteOwnedSearchFilterError::PersistedSearchFilterStateInvalid { .. } => {
                ApiError::internal_server_error(SEARCH_FILTER_INTERNAL_ERROR)
                    .with_detail("Search filter state is invalid.")
            }
            DeleteOwnedSearchFilterError::SearchFilterLookupFailed { .. }
            | DeleteOwnedSearchFilterError::SearchFilterDeletionFailed { .. }
            | DeleteOwnedSearchFilterError::BeginTransactionFailed
            | DeleteOwnedSearchFilterError::CommitTransactionFailed => {
                ApiError::service_unavailable(SEARCH_FILTER_TEMPORARILY_UNAVAILABLE)
                    .with_detail("Search filter could not be deleted right now.")
            }
        }
    }
}

impl From<ListSearchFilterMatchesError> for ApiError {
    fn from(error: ListSearchFilterMatchesError) -> Self {
        match error {
            ListSearchFilterMatchesError::AuthenticatedActorRequired => {
                ApiError::unauthorized(INVALID_CREDENTIALS)
                    .with_header_field("Authorization")
                    .with_detail("Bearer token is required.")
            }
            ListSearchFilterMatchesError::ActorMayNotManageSearchFilter => {
                ApiError::forbidden(FORBIDDEN).with_detail("Operation is not permitted.")
            }
            ListSearchFilterMatchesError::SearchFilterNotFound => {
                ApiError::not_found(SEARCH_FILTER_NOT_FOUND)
                    .with_detail("Search filter was not found.")
            }
            ListSearchFilterMatchesError::ProductDetailsInvalid { .. }
            | ListSearchFilterMatchesError::MatchedProductMissing { .. }
            | ListSearchFilterMatchesError::HiddenProductRedactionFailed { .. } => {
                ApiError::internal_server_error(SEARCH_FILTER_INTERNAL_ERROR)
                    .with_detail("Search filter match product data is invalid.")
            }
            ListSearchFilterMatchesError::SearchFilterMatchReadFailed { .. }
            | ListSearchFilterMatchesError::ProductDetailsReadFailed { .. }
            | ListSearchFilterMatchesError::NotificationReadFailed { .. } => {
                ApiError::service_unavailable(SEARCH_FILTER_TEMPORARILY_UNAVAILABLE)
                    .with_detail("Search filter matches are temporarily unavailable.")
            }
        }
    }
}

impl From<UpdateSearchFilterMatchFeedbackError> for ApiError {
    fn from(error: UpdateSearchFilterMatchFeedbackError) -> Self {
        match error {
            UpdateSearchFilterMatchFeedbackError::AuthenticatedActorRequired => {
                ApiError::unauthorized(INVALID_CREDENTIALS)
                    .with_header_field("Authorization")
                    .with_detail("Bearer token is required.")
            }
            UpdateSearchFilterMatchFeedbackError::ActorMayNotManageSearchFilter => {
                ApiError::forbidden(FORBIDDEN).with_detail("Operation is not permitted.")
            }
            UpdateSearchFilterMatchFeedbackError::SearchFilterNotFound => {
                ApiError::not_found(SEARCH_FILTER_NOT_FOUND)
                    .with_detail("Search filter was not found.")
            }
            UpdateSearchFilterMatchFeedbackError::SearchFilterMatchNotFound => {
                ApiError::not_found(SEARCH_FILTER_MATCH_NOT_FOUND)
                    .with_detail("Search filter match was not found.")
            }
            UpdateSearchFilterMatchFeedbackError::PersistedSearchFilterStateInvalid { .. }
            | UpdateSearchFilterMatchFeedbackError::PersistedSearchFilterMatchStateInvalid {
                ..
            } => ApiError::internal_server_error(SEARCH_FILTER_INTERNAL_ERROR)
                .with_detail("Search filter state is invalid."),
            UpdateSearchFilterMatchFeedbackError::SearchFilterLookupFailed { .. }
            | UpdateSearchFilterMatchFeedbackError::SearchFilterMatchLookupFailed { .. }
            | UpdateSearchFilterMatchFeedbackError::SearchFilterMatchUpdateFailed { .. }
            | UpdateSearchFilterMatchFeedbackError::BeginTransactionFailed
            | UpdateSearchFilterMatchFeedbackError::CommitTransactionFailed => {
                ApiError::service_unavailable(SEARCH_FILTER_TEMPORARILY_UNAVAILABLE)
                    .with_detail("Search filter match could not be updated right now.")
            }
        }
    }
}

impl From<CreateProductError> for ApiError {
    fn from(error: CreateProductError) -> Self {
        match error {
            CreateProductError::AuthenticatedActorRequired => {
                ApiError::unauthorized(INVALID_CREDENTIALS)
                    .with_header_field("Authorization")
                    .with_detail("Bearer token is required.")
            }
            CreateProductError::Forbidden => {
                ApiError::forbidden(FORBIDDEN).with_detail("Operation is not permitted.")
            }
            CreateProductError::ShopNotFound => {
                ApiError::not_found(SHOP_NOT_FOUND).with_detail("Shop was not found.")
            }
            CreateProductError::ShopProductAlreadyExists
            | CreateProductError::ProductSlugAlreadyExists
            | CreateProductError::ProductCurrentEventIdConflict
            | CreateProductError::ProductEventAlreadyExists => {
                ApiError::conflict(CONFLICT).with_detail("Product conflicts with current state.")
            }
            CreateProductError::InvalidProductState => {
                ApiError::bad_request(BAD_BODY_VALUE).with_detail("Product create is invalid.")
            }
            CreateProductError::PartnerProductAuthorizationTemporarilyUnavailable { .. }
            | CreateProductError::ProductLookupByIdFailed
            | CreateProductError::ProductLookupByKeyFailed { .. }
            | CreateProductError::ProductInsertFailed
            | CreateProductError::ProductUpdateFailed
            | CreateProductError::ProductEventAppendFailed
            | CreateProductError::CurrentProductEventLookupFailed
            | CreateProductError::BeginTransactionFailed
            | CreateProductError::CommitTransactionFailed => {
                ApiError::service_unavailable(PRODUCT_TEMPORARILY_UNAVAILABLE)
                    .with_detail("Product create is temporarily unavailable.")
            }
            _ => ApiError::internal_server_error(PRODUCT_INTERNAL_ERROR)
                .with_detail("Product create failed internally."),
        }
    }
}

impl From<UpdateProductError> for ApiError {
    fn from(error: UpdateProductError) -> Self {
        match error {
            UpdateProductError::AuthenticatedActorRequired => {
                ApiError::unauthorized(INVALID_CREDENTIALS)
                    .with_header_field("Authorization")
                    .with_detail("Bearer token is required.")
            }
            UpdateProductError::Forbidden => {
                ApiError::forbidden(FORBIDDEN).with_detail("Operation is not permitted.")
            }
            UpdateProductError::ShopNotFound => {
                ApiError::not_found(SHOP_NOT_FOUND).with_detail("Shop was not found.")
            }
            UpdateProductError::ProductNotFound => {
                ApiError::not_found(PRODUCT_NOT_FOUND).with_detail("Product was not found.")
            }
            UpdateProductError::ProductCurrentEventIdConflict
            | UpdateProductError::ShopProductAlreadyExists
            | UpdateProductError::ProductSlugAlreadyExists
            | UpdateProductError::ProductEventAlreadyExists => {
                ApiError::conflict(CONFLICT).with_detail("Product conflicts with current state.")
            }
            UpdateProductError::StateRequired | UpdateProductError::UrlRequired => {
                ApiError::bad_request(BAD_BODY_VALUE).with_detail("Product update is invalid.")
            }
            UpdateProductError::PartnerProductAuthorizationTemporarilyUnavailable { .. }
            | UpdateProductError::ProductLookupByIdFailed
            | UpdateProductError::ProductLookupByKeyFailed { .. }
            | UpdateProductError::ProductInsertFailed
            | UpdateProductError::ProductUpdateFailed
            | UpdateProductError::ProductEventAppendFailed
            | UpdateProductError::CurrentProductEventLookupFailed
            | UpdateProductError::BeginTransactionFailed
            | UpdateProductError::CommitTransactionFailed => {
                ApiError::service_unavailable(PRODUCT_TEMPORARILY_UNAVAILABLE)
                    .with_detail("Product update is temporarily unavailable.")
            }
            _ => ApiError::internal_server_error(PRODUCT_INTERNAL_ERROR)
                .with_detail("Product update failed internally."),
        }
    }
}

impl From<DeleteProductError> for ApiError {
    fn from(error: DeleteProductError) -> Self {
        match error {
            DeleteProductError::AuthenticatedActorRequired => {
                ApiError::unauthorized(INVALID_CREDENTIALS)
                    .with_header_field("Authorization")
                    .with_detail("Bearer token is required.")
            }
            DeleteProductError::Forbidden => {
                ApiError::forbidden(FORBIDDEN).with_detail("Operation is not permitted.")
            }
            DeleteProductError::ShopNotFound => {
                ApiError::not_found(SHOP_NOT_FOUND).with_detail("Shop was not found.")
            }
            DeleteProductError::ProductNotFound => {
                ApiError::not_found(PRODUCT_NOT_FOUND).with_detail("Product was not found.")
            }
            DeleteProductError::ProductCurrentEventIdConflict
            | DeleteProductError::ShopProductAlreadyExists
            | DeleteProductError::ProductSlugAlreadyExists
            | DeleteProductError::ProductEventAlreadyExists => {
                ApiError::conflict(CONFLICT).with_detail("Product conflicts with current state.")
            }
            DeleteProductError::PartnerProductAuthorizationTemporarilyUnavailable { .. }
            | DeleteProductError::ProductLookupByIdFailed
            | DeleteProductError::ProductLookupByKeyFailed { .. }
            | DeleteProductError::ProductInsertFailed
            | DeleteProductError::ProductUpdateFailed
            | DeleteProductError::ProductEventAppendFailed
            | DeleteProductError::CurrentProductEventLookupFailed
            | DeleteProductError::BeginTransactionFailed
            | DeleteProductError::CommitTransactionFailed => {
                ApiError::service_unavailable(PRODUCT_TEMPORARILY_UNAVAILABLE)
                    .with_detail("Product delete is temporarily unavailable.")
            }
            _ => ApiError::internal_server_error(PRODUCT_INTERNAL_ERROR)
                .with_detail("Product delete failed internally."),
        }
    }
}

impl From<UpsertProductError> for ApiError {
    fn from(error: UpsertProductError) -> Self {
        match error {
            UpsertProductError::AuthenticatedActorRequired => {
                ApiError::unauthorized(INVALID_CREDENTIALS)
                    .with_header_field("Authorization")
                    .with_detail("Bearer token is required.")
            }
            UpsertProductError::Forbidden => {
                ApiError::forbidden(FORBIDDEN).with_detail("Operation is not permitted.")
            }
            UpsertProductError::ShopNotFound => {
                ApiError::not_found(SHOP_NOT_FOUND).with_detail("Shop was not found.")
            }
            UpsertProductError::ProductCurrentEventIdConflict
            | UpsertProductError::ProductKeyAlreadyExists
            | UpsertProductError::ProductSlugAlreadyExists => {
                ApiError::conflict(CONFLICT).with_detail("Product conflicts with current state.")
            }
            UpsertProductError::InvalidProductState => {
                ApiError::bad_request(BAD_BODY_VALUE).with_detail("Product upsert is invalid.")
            }
            UpsertProductError::PartnerProductAuthorizationTemporarilyUnavailable { .. }
            | UpsertProductError::ProductPersistenceTemporarilyUnavailable { .. }
            | UpsertProductError::ProductEventStoreTemporarilyUnavailable { .. }
            | UpsertProductError::BeginTransactionFailed
            | UpsertProductError::CommitTransactionFailed => {
                ApiError::service_unavailable(PRODUCT_TEMPORARILY_UNAVAILABLE)
                    .with_detail("Product upsert is temporarily unavailable.")
            }
            UpsertProductError::PartnerProductAuthorizationInternal { .. }
            | UpsertProductError::InvalidPersistedProductState { .. } => {
                ApiError::internal_server_error(PRODUCT_INTERNAL_ERROR)
                    .with_detail("Product upsert failed internally.")
            }
        }
    }
}

impl From<GetProductError> for ApiError {
    fn from(error: GetProductError) -> Self {
        match error {
            GetProductError::NotFound => {
                ApiError::not_found(PRODUCT_NOT_FOUND).with_detail("Product was not found.")
            }
            GetProductError::ProductDetailsQueryFailed
            | GetProductError::ProductNotificationReadFailed { .. }
            | GetProductError::BeginTransactionFailed
            | GetProductError::CommitTransactionFailed => {
                ApiError::service_unavailable(PRODUCT_TEMPORARILY_UNAVAILABLE)
                    .with_detail("Product details are temporarily unavailable.")
            }
            GetProductError::ProductDetailsReadModelInvalid => {
                ApiError::internal_server_error(PRODUCT_INTERNAL_ERROR)
                    .with_detail("Product details failed internally.")
            }
        }
    }
}

impl From<GetProductEventsError> for ApiError {
    fn from(error: GetProductEventsError) -> Self {
        match error {
            GetProductEventsError::NotFound => {
                ApiError::not_found(PRODUCT_NOT_FOUND).with_detail("Product was not found.")
            }
            GetProductEventsError::ProductEventQueryFailed
            | GetProductEventsError::BeginTransactionFailed
            | GetProductEventsError::CommitTransactionFailed => {
                ApiError::service_unavailable(PRODUCT_TEMPORARILY_UNAVAILABLE)
                    .with_detail("Product history is temporarily unavailable.")
            }
            GetProductEventsError::ProductEventReadModelInvalid => {
                ApiError::internal_server_error(PRODUCT_INTERNAL_ERROR)
                    .with_detail("Product history contains invalid event data.")
            }
        }
    }
}

impl From<GetSimilarProductsError> for ApiError {
    fn from(error: GetSimilarProductsError) -> Self {
        match error {
            GetSimilarProductsError::NotFound => {
                ApiError::not_found(PRODUCT_NOT_FOUND).with_detail("Product was not found.")
            }
            GetSimilarProductsError::ProductEmbeddingQueryFailed { .. }
            | GetSimilarProductsError::SimilaritySearchUnavailable
            | GetSimilarProductsError::BeginTransactionFailed
            | GetSimilarProductsError::CommitTransactionFailed
            | GetSimilarProductsError::ProductUserStateQueryFailed { .. }
            | GetSimilarProductsError::ProductNotificationReadFailed { .. } => {
                ApiError::service_unavailable(PRODUCT_TEMPORARILY_UNAVAILABLE)
                    .with_detail("Similar products are temporarily unavailable.")
            }
            GetSimilarProductsError::ProductUserStateReadModelInvalid { .. }
            | GetSimilarProductsError::ProductUserStateMissing
            | GetSimilarProductsError::HiddenProductSummaryInvalid { .. } => {
                ApiError::internal_server_error(PRODUCT_INTERNAL_ERROR)
                    .with_detail("Similar product personalization failed internally.")
            }
        }
    }
}

impl From<SearchProductsError> for ApiError {
    fn from(error: SearchProductsError) -> Self {
        match error {
            SearchProductsError::ProductSearchQueryFailed
            | SearchProductsError::ProductUserStateQueryFailed { .. }
            | SearchProductsError::ProductNotificationReadFailed { .. } => {
                ApiError::service_unavailable(PRODUCT_TEMPORARILY_UNAVAILABLE)
                    .with_detail("Product search is temporarily unavailable.")
            }
            SearchProductsError::ProductSearchReadModelInvalid
            | SearchProductsError::ProductUserStateReadModelInvalid { .. }
            | SearchProductsError::ProductUserStateMissing
            | SearchProductsError::HiddenProductSummaryInvalid { .. } => {
                ApiError::internal_server_error(PRODUCT_INTERNAL_ERROR)
                    .with_detail("Product search failed internally.")
            }
        }
    }
}

impl From<UpsertNewsletterSubscriptionError> for ApiError {
    fn from(error: UpsertNewsletterSubscriptionError) -> Self {
        match error {
            UpsertNewsletterSubscriptionError::InvalidEmail => ApiError::bad_request(INVALID_EMAIL)
                .with_detail("Newsletter provider rejected the email address."),
            UpsertNewsletterSubscriptionError::NewsletterSubscriptionUnavailable { .. } => {
                ApiError::service_unavailable(NEWSLETTER_TEMPORARILY_UNAVAILABLE)
                    .with_detail("Newsletter subscription is temporarily unavailable.")
            }
            UpsertNewsletterSubscriptionError::NewsletterSubscriptionInternal { .. } => {
                ApiError::internal_server_error(NEWSLETTER_INTERNAL_ERROR)
                    .with_detail("Newsletter subscription failed internally.")
            }
        }
    }
}

impl From<CheckUserAdminError> for ApiError {
    fn from(error: CheckUserAdminError) -> Self {
        match error {
            CheckUserAdminError::AuthenticatedActorRequired => {
                ApiError::unauthorized(INVALID_CREDENTIALS)
                    .with_header_field("Authorization")
                    .with_detail("Bearer token is required.")
            }
            CheckUserAdminError::Forbidden => {
                ApiError::forbidden(FORBIDDEN).with_detail("Operation is not permitted.")
            }
            CheckUserAdminError::TemporarilyUnavailable { .. }
            | CheckUserAdminError::BeginTransactionFailed
            | CheckUserAdminError::CommitTransactionFailed => {
                ApiError::service_unavailable(USER_TEMPORARILY_UNAVAILABLE)
                    .with_detail("User details are temporarily unavailable.")
            }
            CheckUserAdminError::InvalidReadModel { .. } | CheckUserAdminError::Internal { .. } => {
                ApiError::internal_server_error(USER_INTERNAL_ERROR)
                    .with_detail("User details failed internally.")
            }
        }
    }
}

impl From<GetOwnUserError> for ApiError {
    fn from(error: GetOwnUserError) -> Self {
        match error {
            GetOwnUserError::AuthenticatedActorRequired => {
                ApiError::unauthorized(INVALID_CREDENTIALS)
                    .with_header_field("Authorization")
                    .with_detail("Bearer token is required.")
            }
            GetOwnUserError::Forbidden => {
                ApiError::forbidden(FORBIDDEN).with_detail("Operation is not permitted.")
            }
            GetOwnUserError::NotFound => {
                ApiError::not_found(USER_NOT_FOUND).with_detail("User was not found.")
            }
            GetOwnUserError::TemporarilyUnavailable { .. }
            | GetOwnUserError::BeginTransactionFailed
            | GetOwnUserError::CommitTransactionFailed => {
                ApiError::service_unavailable(USER_TEMPORARILY_UNAVAILABLE)
                    .with_detail("User details are temporarily unavailable.")
            }
            GetOwnUserError::InvalidReadModel { .. } | GetOwnUserError::Internal { .. } => {
                ApiError::internal_server_error(USER_INTERNAL_ERROR)
                    .with_detail("User details failed internally.")
            }
        }
    }
}

impl From<AdminGetUserError> for ApiError {
    fn from(error: AdminGetUserError) -> Self {
        match error {
            AdminGetUserError::AuthenticatedActorRequired => {
                ApiError::unauthorized(INVALID_CREDENTIALS)
                    .with_header_field("Authorization")
                    .with_detail("Bearer token is required.")
            }
            AdminGetUserError::Forbidden => {
                ApiError::forbidden(FORBIDDEN).with_detail("Operation is not permitted.")
            }
            AdminGetUserError::NotFound => {
                ApiError::not_found(USER_NOT_FOUND).with_detail("User was not found.")
            }
            AdminGetUserError::TemporarilyUnavailable { .. }
            | AdminGetUserError::BeginTransactionFailed
            | AdminGetUserError::CommitTransactionFailed => {
                ApiError::service_unavailable(USER_TEMPORARILY_UNAVAILABLE)
                    .with_detail("User details are temporarily unavailable.")
            }
            AdminGetUserError::InvalidReadModel { .. } | AdminGetUserError::Internal { .. } => {
                ApiError::internal_server_error(USER_INTERNAL_ERROR)
                    .with_detail("User details failed internally.")
            }
        }
    }
}

impl From<SearchUsersError> for ApiError {
    fn from(error: SearchUsersError) -> Self {
        match error {
            SearchUsersError::AuthenticatedActorRequired => {
                ApiError::unauthorized(INVALID_CREDENTIALS)
                    .with_header_field("Authorization")
                    .with_detail("Bearer token is required.")
            }
            SearchUsersError::Forbidden => {
                ApiError::forbidden(FORBIDDEN).with_detail("Operation is not permitted.")
            }
            SearchUsersError::TemporarilyUnavailable { .. }
            | SearchUsersError::BeginTransactionFailed
            | SearchUsersError::CommitTransactionFailed => {
                ApiError::service_unavailable(USER_TEMPORARILY_UNAVAILABLE)
                    .with_detail("User search is temporarily unavailable.")
            }
            SearchUsersError::InvalidReadModel { .. } | SearchUsersError::Internal { .. } => {
                ApiError::internal_server_error(USER_INTERNAL_ERROR)
                    .with_detail("User search failed internally.")
            }
        }
    }
}

impl From<UpdateUserProfileError> for ApiError {
    fn from(error: UpdateUserProfileError) -> Self {
        match error {
            UpdateUserProfileError::AuthenticatedActorRequired => {
                ApiError::unauthorized(INVALID_CREDENTIALS)
                    .with_header_field("Authorization")
                    .with_detail("Bearer token is required.")
            }
            UpdateUserProfileError::Forbidden => {
                ApiError::forbidden(FORBIDDEN).with_detail("Operation is not permitted.")
            }
            UpdateUserProfileError::UserNotFound => {
                ApiError::not_found(USER_NOT_FOUND).with_detail("User was not found.")
            }
            UpdateUserProfileError::ConcurrencyConflict
            | UpdateUserProfileError::EmailConflict { .. } => ApiError::conflict(CONFLICT)
                .with_detail("User update conflicts with current state."),
            UpdateUserProfileError::EmailRequired
            | UpdateUserProfileError::InvalidUserState { .. } => {
                ApiError::bad_request(BAD_BODY_VALUE).with_detail("User update is invalid.")
            }
            UpdateUserProfileError::TemporarilyUnavailable { .. }
            | UpdateUserProfileError::BeginTransactionFailed
            | UpdateUserProfileError::CommitTransactionFailed => {
                ApiError::service_unavailable(USER_TEMPORARILY_UNAVAILABLE)
                    .with_detail("User could not be updated right now.")
            }
            UpdateUserProfileError::InvalidPersistedState { .. }
            | UpdateUserProfileError::Internal { .. } => {
                ApiError::internal_server_error(USER_INTERNAL_ERROR)
                    .with_detail("User update failed internally.")
            }
        }
    }
}

impl From<ChangeUserRoleError> for ApiError {
    fn from(error: ChangeUserRoleError) -> Self {
        match error {
            ChangeUserRoleError::AuthenticatedActorRequired => {
                ApiError::unauthorized(INVALID_CREDENTIALS)
                    .with_header_field("Authorization")
                    .with_detail("Bearer token is required.")
            }
            ChangeUserRoleError::Forbidden => {
                ApiError::forbidden(FORBIDDEN).with_detail("Operation is not permitted.")
            }
            ChangeUserRoleError::UserNotFound => {
                ApiError::not_found(USER_NOT_FOUND).with_detail("User was not found.")
            }
            ChangeUserRoleError::ConcurrencyConflict
            | ChangeUserRoleError::EmailConflict { .. }
            | ChangeUserRoleError::StripeCustomerConflict { .. } => ApiError::conflict(CONFLICT)
                .with_detail("User update conflicts with current state."),
            ChangeUserRoleError::TemporarilyUnavailable { .. }
            | ChangeUserRoleError::BeginTransactionFailed
            | ChangeUserRoleError::CommitTransactionFailed => {
                ApiError::service_unavailable(USER_TEMPORARILY_UNAVAILABLE)
                    .with_detail("User could not be updated right now.")
            }
            ChangeUserRoleError::InvalidPersistedState { .. }
            | ChangeUserRoleError::Internal { .. } => {
                ApiError::internal_server_error(USER_INTERNAL_ERROR)
                    .with_detail("User update failed internally.")
            }
        }
    }
}

impl From<ChangeUserTierError> for ApiError {
    fn from(error: ChangeUserTierError) -> Self {
        match error {
            ChangeUserTierError::AuthenticatedActorRequired => {
                ApiError::unauthorized(INVALID_CREDENTIALS)
                    .with_header_field("Authorization")
                    .with_detail("Bearer token is required.")
            }
            ChangeUserTierError::Forbidden => {
                ApiError::forbidden(FORBIDDEN).with_detail("Operation is not permitted.")
            }
            ChangeUserTierError::UserNotFound => {
                ApiError::not_found(USER_NOT_FOUND).with_detail("User was not found.")
            }
            ChangeUserTierError::ConcurrencyConflict
            | ChangeUserTierError::EmailConflict { .. }
            | ChangeUserTierError::StripeCustomerConflict { .. } => ApiError::conflict(CONFLICT)
                .with_detail("User update conflicts with current state."),
            ChangeUserTierError::TemporarilyUnavailable { .. }
            | ChangeUserTierError::TierEntitlementsLockFailed { .. }
            | ChangeUserTierError::TierEntitlementsReconciliationFailed { .. }
            | ChangeUserTierError::BeginTransactionFailed
            | ChangeUserTierError::CommitTransactionFailed => {
                ApiError::service_unavailable(USER_TEMPORARILY_UNAVAILABLE)
                    .with_detail("User could not be updated right now.")
            }
            ChangeUserTierError::InvalidPersistedState { .. }
            | ChangeUserTierError::Internal { .. } => {
                ApiError::internal_server_error(USER_INTERNAL_ERROR)
                    .with_detail("User update failed internally.")
            }
        }
    }
}

impl From<DeleteUserError> for ApiError {
    fn from(error: DeleteUserError) -> Self {
        match error {
            DeleteUserError::AuthenticatedActorRequired => {
                ApiError::unauthorized(INVALID_CREDENTIALS)
                    .with_header_field("Authorization")
                    .with_detail("Bearer token is required.")
            }
            DeleteUserError::Forbidden => {
                ApiError::forbidden(FORBIDDEN).with_detail("Operation is not permitted.")
            }
            DeleteUserError::UserNotFound => {
                ApiError::not_found(USER_NOT_FOUND).with_detail("User was not found.")
            }
            DeleteUserError::ConcurrencyConflict
            | DeleteUserError::EmailConflict { .. }
            | DeleteUserError::StripeCustomerConflict { .. } => ApiError::conflict(CONFLICT)
                .with_detail("User delete conflicts with current state."),
            DeleteUserError::TemporarilyUnavailable { .. }
            | DeleteUserError::BeginTransactionFailed
            | DeleteUserError::CommitTransactionFailed => {
                ApiError::service_unavailable(USER_TEMPORARILY_UNAVAILABLE)
                    .with_detail("User could not be unwatched right now.")
            }
            DeleteUserError::InvalidPersistedState { .. } | DeleteUserError::Internal { .. } => {
                ApiError::internal_server_error(USER_INTERNAL_ERROR)
                    .with_detail("User delete failed internally.")
            }
        }
    }
}

impl From<CreateAccessTokenError> for ApiError {
    fn from(error: CreateAccessTokenError) -> Self {
        match error {
            CreateAccessTokenError::AuthenticatedActorRequired => {
                ApiError::unauthorized(INVALID_CREDENTIALS)
                    .with_header_field("Authorization")
                    .with_detail("Bearer token is required.")
            }
            CreateAccessTokenError::Forbidden => {
                ApiError::forbidden(FORBIDDEN).with_detail("Operation is not permitted.")
            }
            CreateAccessTokenError::Conflict { .. } => {
                ApiError::conflict(CONFLICT).with_detail("Access token already exists.")
            }
            CreateAccessTokenError::TemporarilyUnavailable { .. } => {
                ApiError::service_unavailable(ACCESS_TOKEN_TEMPORARILY_UNAVAILABLE)
                    .with_detail("Access token store is temporarily unavailable.")
            }
            CreateAccessTokenError::InvalidPersistedState { .. }
            | CreateAccessTokenError::Internal { .. } => {
                ApiError::internal_server_error(ACCESS_TOKEN_INTERNAL_ERROR)
                    .with_detail("Access token operation failed internally.")
            }
        }
    }
}
impl From<ListAccessTokensError> for ApiError {
    fn from(error: ListAccessTokensError) -> Self {
        match error {
            ListAccessTokensError::AuthenticatedActorRequired => {
                ApiError::unauthorized(INVALID_CREDENTIALS)
                    .with_header_field("Authorization")
                    .with_detail("Bearer token is required.")
            }
            ListAccessTokensError::Forbidden => {
                ApiError::forbidden(FORBIDDEN).with_detail("Operation is not permitted.")
            }
            ListAccessTokensError::Conflict { .. } => {
                ApiError::conflict(CONFLICT).with_detail("Access token conflict.")
            }
            ListAccessTokensError::TemporarilyUnavailable { .. } => {
                ApiError::service_unavailable(ACCESS_TOKEN_TEMPORARILY_UNAVAILABLE)
                    .with_detail("Access token store is temporarily unavailable.")
            }
            ListAccessTokensError::InvalidPersistedState { .. }
            | ListAccessTokensError::Internal { .. } => {
                ApiError::internal_server_error(ACCESS_TOKEN_INTERNAL_ERROR)
                    .with_detail("Access token operation failed internally.")
            }
        }
    }
}
impl From<GetAccessTokenError> for ApiError {
    fn from(error: GetAccessTokenError) -> Self {
        match error {
            GetAccessTokenError::AuthenticatedActorRequired => {
                ApiError::unauthorized(INVALID_CREDENTIALS)
                    .with_header_field("Authorization")
                    .with_detail("Bearer token is required.")
            }
            GetAccessTokenError::Forbidden => {
                ApiError::forbidden(FORBIDDEN).with_detail("Operation is not permitted.")
            }
            GetAccessTokenError::NotFound => ApiError::not_found(ACCESS_TOKEN_NOT_FOUND)
                .with_detail("Access token was not found."),
            GetAccessTokenError::Conflict { .. } => {
                ApiError::conflict(CONFLICT).with_detail("Access token conflict.")
            }
            GetAccessTokenError::TemporarilyUnavailable { .. } => {
                ApiError::service_unavailable(ACCESS_TOKEN_TEMPORARILY_UNAVAILABLE)
                    .with_detail("Access token store is temporarily unavailable.")
            }
            GetAccessTokenError::InvalidPersistedState { .. }
            | GetAccessTokenError::Internal { .. } => {
                ApiError::internal_server_error(ACCESS_TOKEN_INTERNAL_ERROR)
                    .with_detail("Access token operation failed internally.")
            }
        }
    }
}
impl From<UpdateAccessTokenError> for ApiError {
    fn from(error: UpdateAccessTokenError) -> Self {
        match error {
            UpdateAccessTokenError::AuthenticatedActorRequired => {
                ApiError::unauthorized(INVALID_CREDENTIALS)
                    .with_header_field("Authorization")
                    .with_detail("Bearer token is required.")
            }
            UpdateAccessTokenError::Forbidden => {
                ApiError::forbidden(FORBIDDEN).with_detail("Operation is not permitted.")
            }
            UpdateAccessTokenError::AccessTokenNotFound => {
                ApiError::not_found(ACCESS_TOKEN_NOT_FOUND)
                    .with_detail("Access token was not found.")
            }
            UpdateAccessTokenError::NameRequired => {
                ApiError::bad_request(BAD_BODY_VALUE).with_detail("Access token name is required.")
            }
            UpdateAccessTokenError::Conflict { .. } => {
                ApiError::conflict(CONFLICT).with_detail("Access token conflict.")
            }
            UpdateAccessTokenError::TemporarilyUnavailable { .. } => {
                ApiError::service_unavailable(ACCESS_TOKEN_TEMPORARILY_UNAVAILABLE)
                    .with_detail("Access token store is temporarily unavailable.")
            }
            UpdateAccessTokenError::InvalidPersistedState { .. }
            | UpdateAccessTokenError::Internal { .. } => {
                ApiError::internal_server_error(ACCESS_TOKEN_INTERNAL_ERROR)
                    .with_detail("Access token operation failed internally.")
            }
        }
    }
}
impl From<DeleteAccessTokenError> for ApiError {
    fn from(error: DeleteAccessTokenError) -> Self {
        match error {
            DeleteAccessTokenError::AuthenticatedActorRequired => {
                ApiError::unauthorized(INVALID_CREDENTIALS)
                    .with_header_field("Authorization")
                    .with_detail("Bearer token is required.")
            }
            DeleteAccessTokenError::Forbidden => {
                ApiError::forbidden(FORBIDDEN).with_detail("Operation is not permitted.")
            }
            DeleteAccessTokenError::Conflict { .. } => {
                ApiError::conflict(CONFLICT).with_detail("Access token conflict.")
            }
            DeleteAccessTokenError::TemporarilyUnavailable { .. } => {
                ApiError::service_unavailable(ACCESS_TOKEN_TEMPORARILY_UNAVAILABLE)
                    .with_detail("Access token store is temporarily unavailable.")
            }
            DeleteAccessTokenError::InvalidPersistedState { .. }
            | DeleteAccessTokenError::Internal { .. } => {
                ApiError::internal_server_error(ACCESS_TOKEN_INTERNAL_ERROR)
                    .with_detail("Access token operation failed internally.")
            }
        }
    }
}

impl From<ListWatchlistError> for ApiError {
    fn from(error: ListWatchlistError) -> Self {
        match error {
            ListWatchlistError::AuthenticatedActorRequired => {
                ApiError::unauthorized(INVALID_CREDENTIALS)
                    .with_header_field("Authorization")
                    .with_detail("Bearer token is required.")
            }
            ListWatchlistError::Forbidden => {
                ApiError::forbidden(FORBIDDEN).with_detail("Operation is not permitted.")
            }
            ListWatchlistError::TemporarilyUnavailable
            | ListWatchlistError::NotificationReadFailed { .. }
            | ListWatchlistError::BeginTransactionFailed
            | ListWatchlistError::CommitTransactionFailed => {
                ApiError::service_unavailable(WATCHLIST_TEMPORARILY_UNAVAILABLE)
                    .with_detail("Watchlist is temporarily unavailable.")
            }
            ListWatchlistError::InvalidPersistedState => {
                ApiError::internal_server_error(WATCHLIST_INTERNAL_ERROR)
                    .with_detail("Watchlist failed internally.")
            }
        }
    }
}
impl From<WatchProductError> for ApiError {
    fn from(error: WatchProductError) -> Self {
        match error {
            WatchProductError::AuthenticatedActorRequired => {
                ApiError::unauthorized(INVALID_CREDENTIALS)
                    .with_header_field("Authorization")
                    .with_detail("Bearer token is required.")
            }
            WatchProductError::Forbidden => {
                ApiError::forbidden(FORBIDDEN).with_detail("Operation is not permitted.")
            }
            WatchProductError::AlreadyExists => {
                ApiError::conflict(CONFLICT).with_detail("Watchlist entry already exists.")
            }
            WatchProductError::UserNotFound => {
                ApiError::not_found(USER_NOT_FOUND).with_detail("User was not found.")
            }
            WatchProductError::WatchlistQuotaExceeded {
                active_count,
                quota,
            } => ApiError::unprocessable_content(WATCHLIST_QUOTA_EXCEEDED).with_detail(format!(
                "Exceeded the maximum amount of watchlist entries. There are already {active_count}/{quota} active watchlist entries occupied."
            )),
            WatchProductError::TemporarilyUnavailable
            | WatchProductError::UserTierEntitlementsLockFailed { .. }
            | WatchProductError::WatchlistQuotaReadFailed { .. }
            | WatchProductError::BeginTransactionFailed
            | WatchProductError::CommitTransactionFailed => {
                ApiError::service_unavailable(WATCHLIST_TEMPORARILY_UNAVAILABLE)
                    .with_detail("Watchlist is temporarily unavailable.")
            }
            WatchProductError::InvalidPersistedState => {
                ApiError::internal_server_error(WATCHLIST_INTERNAL_ERROR)
                    .with_detail("Watchlist failed internally.")
            }
        }
    }
}
impl From<UpdateWatchlistProductError> for ApiError {
    fn from(error: UpdateWatchlistProductError) -> Self {
        match error {
            UpdateWatchlistProductError::AuthenticatedActorRequired => {
                ApiError::unauthorized(INVALID_CREDENTIALS)
                    .with_header_field("Authorization")
                    .with_detail("Bearer token is required.")
            }
            UpdateWatchlistProductError::Forbidden => {
                ApiError::forbidden(FORBIDDEN).with_detail("Operation is not permitted.")
            }
            UpdateWatchlistProductError::NotFound => ApiError::not_found(WATCHLIST_ENTRY_NOT_FOUND)
                .with_detail("Watchlist entry was not found."),
            UpdateWatchlistProductError::UserNotFound => {
                ApiError::not_found(USER_NOT_FOUND).with_detail("User was not found.")
            }
            UpdateWatchlistProductError::WatchlistQuotaExceeded {
                active_count,
                quota,
            } => ApiError::unprocessable_content(WATCHLIST_QUOTA_EXCEEDED).with_detail(format!(
                "Exceeded the maximum amount of watchlist entries. There are already {active_count}/{quota} active watchlist entries occupied."
            )),
            UpdateWatchlistProductError::TemporarilyUnavailable
            | UpdateWatchlistProductError::UserTierEntitlementsLockFailed { .. }
            | UpdateWatchlistProductError::WatchlistQuotaReadFailed { .. }
            | UpdateWatchlistProductError::BeginTransactionFailed
            | UpdateWatchlistProductError::CommitTransactionFailed => {
                ApiError::service_unavailable(WATCHLIST_TEMPORARILY_UNAVAILABLE)
                    .with_detail("Watchlist is temporarily unavailable.")
            }
            UpdateWatchlistProductError::InvalidPersistedState => {
                ApiError::internal_server_error(WATCHLIST_INTERNAL_ERROR)
                    .with_detail("Watchlist failed internally.")
            }
        }
    }
}
impl From<UnwatchProductError> for ApiError {
    fn from(error: UnwatchProductError) -> Self {
        match error {
            UnwatchProductError::AuthenticatedActorRequired => {
                ApiError::unauthorized(INVALID_CREDENTIALS)
                    .with_header_field("Authorization")
                    .with_detail("Bearer token is required.")
            }
            UnwatchProductError::Forbidden => {
                ApiError::forbidden(FORBIDDEN).with_detail("Operation is not permitted.")
            }
            UnwatchProductError::NotFound => ApiError::not_found(WATCHLIST_ENTRY_NOT_FOUND)
                .with_detail("Watchlist entry was not found."),
            UnwatchProductError::TemporarilyUnavailable
            | UnwatchProductError::BeginTransactionFailed
            | UnwatchProductError::CommitTransactionFailed => {
                ApiError::service_unavailable(WATCHLIST_TEMPORARILY_UNAVAILABLE)
                    .with_detail("Watchlist is temporarily unavailable.")
            }
            UnwatchProductError::InvalidPersistedState => {
                ApiError::internal_server_error(WATCHLIST_INTERNAL_ERROR)
                    .with_detail("Watchlist failed internally.")
            }
        }
    }
}

impl From<CreatePartnerShopApplicationError> for ApiError {
    fn from(error: CreatePartnerShopApplicationError) -> Self {
        match error {
            CreatePartnerShopApplicationError::AuthenticatedActorRequired => {
                ApiError::unauthorized(INVALID_CREDENTIALS)
                    .with_header_field("Authorization")
                    .with_detail("Bearer token is required.")
            }
            CreatePartnerShopApplicationError::Forbidden => {
                ApiError::forbidden(FORBIDDEN).with_detail("Operation is not permitted.")
            }
            CreatePartnerShopApplicationError::ShopNotFound => {
                ApiError::not_found(SHOP_NOT_FOUND).with_detail("Shop was not found.")
            }
            CreatePartnerShopApplicationError::ShopNotEligible => ApiError::conflict(CONFLICT)
                .with_detail("Shop is not eligible for a partner application."),
            CreatePartnerShopApplicationError::SlugConflict { .. } => {
                ApiError::conflict(SHOP_EXISTS_ALREADY).with_detail("Shop exists already.")
            }
            CreatePartnerShopApplicationError::InvalidAddress => {
                ApiError::bad_request(BAD_BODY_VALUE).with_detail("Shop address is invalid.")
            }
            CreatePartnerShopApplicationError::TemporarilyUnavailable { .. }
            | CreatePartnerShopApplicationError::BeginTransactionFailed
            | CreatePartnerShopApplicationError::CommitTransactionFailed => {
                ApiError::service_unavailable(PARTNER_SHOP_APPLICATION_TEMPORARILY_UNAVAILABLE)
                    .with_detail("Partner shop application is temporarily unavailable.")
            }
            CreatePartnerShopApplicationError::InvalidPersistedState { .. }
            | CreatePartnerShopApplicationError::Internal { .. } => {
                ApiError::internal_server_error(PARTNER_SHOP_APPLICATION_INTERNAL_ERROR)
                    .with_detail("Partner shop application failed internally.")
            }
        }
    }
}
macro_rules! impl_partner_shop_application_error {
    ($error:ident, $not_found:ident, $conflict:ident) => {
        impl From<$error> for ApiError {
            fn from(error: $error) -> Self {
                match error {
                    $error::Forbidden => {
                        ApiError::forbidden(FORBIDDEN).with_detail("Operation is not permitted.")
                    }
                    $error::$not_found => ApiError::not_found(PARTNER_SHOP_APPLICATION_NOT_FOUND)
                        .with_detail("Partner shop application was not found."),
                    $error::$conflict => ApiError::conflict(CONFLICT)
                        .with_detail("Partner shop application was changed concurrently."),
                    $error::TemporarilyUnavailable { .. }
                    | $error::BeginTransactionFailed
                    | $error::CommitTransactionFailed => ApiError::service_unavailable(
                        PARTNER_SHOP_APPLICATION_TEMPORARILY_UNAVAILABLE,
                    )
                    .with_detail("Partner shop application is temporarily unavailable."),
                    $error::InvalidPersistedState { .. } | $error::Internal { .. } => {
                        ApiError::internal_server_error(PARTNER_SHOP_APPLICATION_INTERNAL_ERROR)
                            .with_detail("Partner shop application failed internally.")
                    }
                }
            }
        }
    };
    ($error:ident) => {
        impl From<$error> for ApiError {
            fn from(error: $error) -> Self {
                match error {
                    $error::Forbidden => {
                        ApiError::forbidden(FORBIDDEN).with_detail("Operation is not permitted.")
                    }
                    $error::TemporarilyUnavailable { .. }
                    | $error::BeginTransactionFailed
                    | $error::CommitTransactionFailed => ApiError::service_unavailable(
                        PARTNER_SHOP_APPLICATION_TEMPORARILY_UNAVAILABLE,
                    )
                    .with_detail("Partner shop application is temporarily unavailable."),
                    $error::InvalidPersistedState { .. } | $error::Internal { .. } => {
                        ApiError::internal_server_error(PARTNER_SHOP_APPLICATION_INTERNAL_ERROR)
                            .with_detail("Partner shop application failed internally.")
                    }
                }
            }
        }
    };
}

impl_partner_shop_application_error!(ListPartnerShopApplicationsError);
impl_partner_shop_application_error!(AdminListPartnerShopApplicationsError);
impl_partner_shop_application_error!(
    GetPartnerShopApplicationError,
    NotFound,
    ConcurrencyConflict
);
impl From<WithdrawPartnerShopApplicationError> for ApiError {
    fn from(error: WithdrawPartnerShopApplicationError) -> Self {
        match error {
            WithdrawPartnerShopApplicationError::Forbidden => {
                ApiError::forbidden(FORBIDDEN).with_detail("Operation is not permitted.")
            }
            WithdrawPartnerShopApplicationError::NotFound => {
                ApiError::not_found(PARTNER_SHOP_APPLICATION_NOT_FOUND)
                    .with_detail("Partner shop application was not found.")
            }
            WithdrawPartnerShopApplicationError::ApplicationNotWithdrawable
            | WithdrawPartnerShopApplicationError::ConcurrencyConflict => ApiError::conflict(
                CONFLICT,
            )
            .with_detail("Partner shop application cannot be withdrawn in its current state."),
            WithdrawPartnerShopApplicationError::TemporarilyUnavailable { .. }
            | WithdrawPartnerShopApplicationError::BeginTransactionFailed
            | WithdrawPartnerShopApplicationError::CommitTransactionFailed => {
                ApiError::service_unavailable(PARTNER_SHOP_APPLICATION_TEMPORARILY_UNAVAILABLE)
                    .with_detail("Partner shop application is temporarily unavailable.")
            }
            WithdrawPartnerShopApplicationError::ShopNotFound
            | WithdrawPartnerShopApplicationError::DraftShopNotDiscardable
            | WithdrawPartnerShopApplicationError::InvalidPersistedState { .. }
            | WithdrawPartnerShopApplicationError::Internal { .. } => {
                ApiError::internal_server_error(PARTNER_SHOP_APPLICATION_INTERNAL_ERROR)
                    .with_detail("Partner shop application failed internally.")
            }
        }
    }
}
impl_partner_shop_application_error!(
    AdminGetPartnerShopApplicationError,
    NotFound,
    ConcurrencyConflict
);
impl From<AdminUpdatePartnerShopApplicationError> for ApiError {
    fn from(error: AdminUpdatePartnerShopApplicationError) -> Self {
        match error {
            AdminUpdatePartnerShopApplicationError::Forbidden => {
                ApiError::forbidden(FORBIDDEN).with_detail("Operation is not permitted.")
            }
            AdminUpdatePartnerShopApplicationError::NotFound => {
                ApiError::not_found(PARTNER_SHOP_APPLICATION_NOT_FOUND)
                    .with_detail("Partner shop application was not found.")
            }
            AdminUpdatePartnerShopApplicationError::ApplicationNotReviewable
            | AdminUpdatePartnerShopApplicationError::ConcurrencyConflict => ApiError::conflict(
                CONFLICT,
            )
            .with_detail("Partner shop application cannot enter review in its current state."),
            AdminUpdatePartnerShopApplicationError::TemporarilyUnavailable { .. }
            | AdminUpdatePartnerShopApplicationError::BeginTransactionFailed
            | AdminUpdatePartnerShopApplicationError::CommitTransactionFailed => {
                ApiError::service_unavailable(PARTNER_SHOP_APPLICATION_TEMPORARILY_UNAVAILABLE)
                    .with_detail("Partner shop application is temporarily unavailable.")
            }
            AdminUpdatePartnerShopApplicationError::InvalidPersistedState { .. }
            | AdminUpdatePartnerShopApplicationError::Internal { .. } => {
                ApiError::internal_server_error(PARTNER_SHOP_APPLICATION_INTERNAL_ERROR)
                    .with_detail("Partner shop application failed internally.")
            }
        }
    }
}
impl From<AdminDecidePartnerShopApplicationError> for ApiError {
    fn from(error: AdminDecidePartnerShopApplicationError) -> Self {
        match error {
            AdminDecidePartnerShopApplicationError::Forbidden => {
                ApiError::forbidden(FORBIDDEN).with_detail("Operation is not permitted.")
            }
            AdminDecidePartnerShopApplicationError::NotFound => {
                ApiError::not_found(PARTNER_SHOP_APPLICATION_NOT_FOUND)
                    .with_detail("Partner shop application was not found.")
            }
            AdminDecidePartnerShopApplicationError::ApplicationNotDecidable
            | AdminDecidePartnerShopApplicationError::ConcurrencyConflict => {
                ApiError::conflict(CONFLICT)
                    .with_detail("Partner shop application cannot be decided in its current state.")
            }
            AdminDecidePartnerShopApplicationError::NotificationFailed { .. }
            | AdminDecidePartnerShopApplicationError::TemporarilyUnavailable { .. }
            | AdminDecidePartnerShopApplicationError::BeginTransactionFailed
            | AdminDecidePartnerShopApplicationError::CommitTransactionFailed => {
                ApiError::service_unavailable(PARTNER_SHOP_APPLICATION_TEMPORARILY_UNAVAILABLE)
                    .with_detail("Partner shop application is temporarily unavailable.")
            }
            AdminDecidePartnerShopApplicationError::ShopNotFound
            | AdminDecidePartnerShopApplicationError::ShopNotPublishable
            | AdminDecidePartnerShopApplicationError::DraftShopNotDiscardable
            | AdminDecidePartnerShopApplicationError::InvalidPersistedState { .. }
            | AdminDecidePartnerShopApplicationError::Internal { .. } => {
                ApiError::internal_server_error(PARTNER_SHOP_APPLICATION_INTERNAL_ERROR)
                    .with_detail("Partner shop application failed internally.")
            }
        }
    }
}

impl From<OAuthServiceError> for ApiError {
    fn from(error: OAuthServiceError) -> Self {
        match error {
            OAuthServiceError::AuthenticatedActorRequired
            | OAuthServiceError::InvalidClientSecret => {
                ApiError::unauthorized(OAUTH_INVALID_CLIENT_SECRET).with_detail(error.to_string())
            }
            OAuthServiceError::Forbidden => ApiError::forbidden(FORBIDDEN),
            OAuthServiceError::ClientNotFound => ApiError::not_found(OAUTH_CLIENT_NOT_FOUND),
            OAuthServiceError::InvalidRedirectUri => {
                ApiError::bad_request(OAUTH_INVALID_REDIRECT_URI)
            }
            OAuthServiceError::InvalidScope => ApiError::bad_request(OAUTH_INVALID_SCOPE),
            OAuthServiceError::AuthorizationCodeNotFound => {
                ApiError::bad_request(OAUTH_AUTHORIZATION_CODE_NOT_FOUND)
            }
            OAuthServiceError::AuthorizationCodeExpired => {
                ApiError::bad_request(OAUTH_AUTHORIZATION_CODE_EXPIRED)
            }
            OAuthServiceError::AuthorizationCodeClientMismatch
            | OAuthServiceError::AuthorizationCodeRedirectUriMismatch => {
                ApiError::bad_request(OAUTH_AUTHORIZATION_CODE_NOT_FOUND)
            }
            OAuthServiceError::InvalidCodeVerifier => {
                ApiError::bad_request(OAUTH_INVALID_CODE_VERIFIER)
            }
            OAuthServiceError::ThirdPartyExchangeCodeNotFound
            | OAuthServiceError::ThirdPartyExchangeCodeExpired => {
                ApiError::bad_request(OAUTH_THIRD_PARTY_EXCHANGE_CODE_NOT_FOUND)
            }
            OAuthServiceError::InvalidClientMetadata(detail) => {
                ApiError::bad_request(OAUTH_INVALID_CLIENT_METADATA).with_detail(detail)
            }
            OAuthServiceError::TemporarilyUnavailable => {
                ApiError::service_unavailable(OAUTH_TEMPORARILY_UNAVAILABLE)
            }
            OAuthServiceError::InvalidPersistedState | OAuthServiceError::Internal => {
                ApiError::internal_server_error(OAUTH_INTERNAL_ERROR)
            }
        }
    }
}

impl From<GetShopError> for ApiError {
    fn from(error: GetShopError) -> Self {
        match error {
            GetShopError::NotFound => {
                ApiError::not_found(SHOP_NOT_FOUND).with_detail("Shop was not found.")
            }
            GetShopError::TemporarilyUnavailable { .. }
            | GetShopError::BeginTransactionFailed
            | GetShopError::CommitTransactionFailed => {
                ApiError::service_unavailable(SHOP_TEMPORARILY_UNAVAILABLE)
                    .with_detail("Shop details are temporarily unavailable.")
            }
            GetShopError::InvalidReadModel { .. } | GetShopError::Internal { .. } => {
                ApiError::internal_server_error(SHOP_INTERNAL_ERROR)
                    .with_detail("Shop details failed internally.")
            }
        }
    }
}

impl From<SearchShopsError> for ApiError {
    fn from(error: SearchShopsError) -> Self {
        match error {
            SearchShopsError::TemporarilyUnavailable { .. }
            | SearchShopsError::BeginTransactionFailed
            | SearchShopsError::CommitTransactionFailed => {
                ApiError::service_unavailable(SHOP_TEMPORARILY_UNAVAILABLE)
                    .with_detail("Shop search is temporarily unavailable.")
            }
            SearchShopsError::InvalidReadModel { .. } | SearchShopsError::Internal { .. } => {
                ApiError::internal_server_error(SHOP_INTERNAL_ERROR)
                    .with_detail("Shop search failed internally.")
            }
        }
    }
}

impl From<CreateShopError> for ApiError {
    fn from(error: CreateShopError) -> Self {
        match error {
            CreateShopError::AuthenticatedActorRequired => {
                ApiError::unauthorized(INVALID_CREDENTIALS)
                    .with_header_field("Authorization")
                    .with_detail("Bearer token is required.")
            }
            CreateShopError::Forbidden => {
                ApiError::forbidden(FORBIDDEN).with_detail("Operation is not permitted.")
            }
            CreateShopError::SlugConflict { .. } => {
                ApiError::conflict(SHOP_EXISTS_ALREADY).with_detail("Shop exists already.")
            }
            CreateShopError::InvalidAddress => {
                ApiError::bad_request(BAD_BODY_VALUE).with_detail("Shop address is invalid.")
            }
            CreateShopError::TemporarilyUnavailable { .. }
            | CreateShopError::BeginTransactionFailed
            | CreateShopError::CommitTransactionFailed => {
                ApiError::service_unavailable(SHOP_TEMPORARILY_UNAVAILABLE)
                    .with_detail("Shop could not be saved right now.")
            }
            CreateShopError::InvalidPersistedState { .. } | CreateShopError::Internal { .. } => {
                ApiError::internal_server_error(SHOP_INTERNAL_ERROR)
                    .with_detail("Shop creation failed internally.")
            }
        }
    }
}

impl From<UpdateShopError> for ApiError {
    fn from(error: UpdateShopError) -> Self {
        match error {
            UpdateShopError::AuthenticatedActorRequired => {
                ApiError::unauthorized(INVALID_CREDENTIALS)
                    .with_header_field("Authorization")
                    .with_detail("Bearer token is required.")
            }
            UpdateShopError::Forbidden => {
                ApiError::forbidden(FORBIDDEN).with_detail("Operation is not permitted.")
            }
            UpdateShopError::ShopNotFound => {
                ApiError::not_found(SHOP_NOT_FOUND).with_detail("Shop was not found.")
            }
            UpdateShopError::ConcurrencyConflict => {
                ApiError::conflict(CONFLICT).with_detail("Shop was changed concurrently.")
            }
            UpdateShopError::SlugConflict { .. } => {
                ApiError::conflict(SHOP_EXISTS_ALREADY).with_detail("Shop exists already.")
            }
            UpdateShopError::ShopTypeRequired
            | UpdateShopError::DomainsRequired
            | UpdateShopError::ShopifyDomainRequired
            | UpdateShopError::InvalidAddress => {
                ApiError::bad_request(BAD_BODY_VALUE).with_detail("Shop update is invalid.")
            }
            UpdateShopError::TemporarilyUnavailable { .. }
            | UpdateShopError::BeginTransactionFailed
            | UpdateShopError::CommitTransactionFailed => {
                ApiError::service_unavailable(SHOP_TEMPORARILY_UNAVAILABLE)
                    .with_detail("Shop could not be updated right now.")
            }
            UpdateShopError::InvalidPersistedState { .. } | UpdateShopError::Internal { .. } => {
                ApiError::internal_server_error(SHOP_INTERNAL_ERROR)
                    .with_detail("Shop update failed internally.")
            }
        }
    }
}

impl From<ListUserPartnerShopsError> for ApiError {
    fn from(error: ListUserPartnerShopsError) -> Self {
        match error {
            ListUserPartnerShopsError::Forbidden => {
                ApiError::forbidden(FORBIDDEN).with_detail("Operation is not permitted.")
            }
            ListUserPartnerShopsError::TemporarilyUnavailable { .. }
            | ListUserPartnerShopsError::BeginTransactionFailed
            | ListUserPartnerShopsError::CommitTransactionFailed => {
                ApiError::service_unavailable(SHOP_TEMPORARILY_UNAVAILABLE)
                    .with_detail("Partner shop details are temporarily unavailable.")
            }
            ListUserPartnerShopsError::InvalidReadModel { .. }
            | ListUserPartnerShopsError::Internal { .. } => {
                ApiError::internal_server_error(SHOP_INTERNAL_ERROR)
                    .with_detail("Partner shop details failed internally.")
            }
        }
    }
}

impl From<CheckUserPartnerShopError> for ApiError {
    fn from(error: CheckUserPartnerShopError) -> Self {
        match error {
            CheckUserPartnerShopError::Forbidden => {
                ApiError::forbidden(FORBIDDEN).with_detail("Operation is not permitted.")
            }
            CheckUserPartnerShopError::TemporarilyUnavailable { .. }
            | CheckUserPartnerShopError::BeginTransactionFailed
            | CheckUserPartnerShopError::CommitTransactionFailed => {
                ApiError::service_unavailable(SHOP_TEMPORARILY_UNAVAILABLE)
                    .with_detail("Partner shop details are temporarily unavailable.")
            }
            CheckUserPartnerShopError::InvalidReadModel { .. }
            | CheckUserPartnerShopError::Internal { .. } => {
                ApiError::internal_server_error(SHOP_INTERNAL_ERROR)
                    .with_detail("Partner shop details failed internally.")
            }
        }
    }
}

impl From<CreateBillingCheckoutSessionError> for ApiError {
    fn from(error: CreateBillingCheckoutSessionError) -> Self {
        match error {
            CreateBillingCheckoutSessionError::AuthenticatedActorRequired => {
                ApiError::unauthorized(INVALID_CREDENTIALS)
                    .with_header_field("Authorization")
                    .with_detail("Bearer token is required.")
            }
            CreateBillingCheckoutSessionError::Forbidden => {
                ApiError::forbidden(FORBIDDEN).with_detail("Operation is not permitted.")
            }
            CreateBillingCheckoutSessionError::UserNotFound => {
                ApiError::not_found(USER_NOT_FOUND).with_detail("User was not found.")
            }
            CreateBillingCheckoutSessionError::StripeCustomerAlreadyExists => {
                ApiError::conflict(STRIPE_CUSTOMER_ALREADY_EXISTS)
                    .with_detail("A Stripe customer is already associated with this user.")
            }
            CreateBillingCheckoutSessionError::StripeCustomerAssociationConflict => {
                ApiError::conflict(STRIPE_CUSTOMER_ASSOCIATION_CONFLICT)
                    .with_detail("Stripe customer association conflicts with current user state.")
            }
            CreateBillingCheckoutSessionError::ConcurrencyConflict => {
                ApiError::conflict(CONFLICT).with_detail("Billing state changed concurrently.")
            }
            CreateBillingCheckoutSessionError::TemporarilyUnavailable { .. } => {
                ApiError::service_unavailable(BILLING_TEMPORARILY_UNAVAILABLE)
                    .with_detail("Billing is temporarily unavailable.")
            }
            CreateBillingCheckoutSessionError::ProviderRejected { .. }
            | CreateBillingCheckoutSessionError::ProviderInvalidResponse { .. } => {
                ApiError::internal_server_error(BILLING_PROVIDER_FAILURE)
                    .with_detail("Billing provider could not create a session.")
            }
            CreateBillingCheckoutSessionError::Internal { .. } => {
                ApiError::internal_server_error(BILLING_INTERNAL_ERROR)
                    .with_detail("Billing failed internally.")
            }
        }
    }
}

impl From<CreateBillingPortalSessionError> for ApiError {
    fn from(error: CreateBillingPortalSessionError) -> Self {
        match error {
            CreateBillingPortalSessionError::AuthenticatedActorRequired => {
                ApiError::unauthorized(INVALID_CREDENTIALS)
                    .with_header_field("Authorization")
                    .with_detail("Bearer token is required.")
            }
            CreateBillingPortalSessionError::Forbidden => {
                ApiError::forbidden(FORBIDDEN).with_detail("Operation is not permitted.")
            }
            CreateBillingPortalSessionError::UserNotFound => {
                ApiError::not_found(USER_NOT_FOUND).with_detail("User was not found.")
            }
            CreateBillingPortalSessionError::StripeCustomerDoesNotExist => {
                ApiError::unprocessable_content(STRIPE_CUSTOMER_DOES_NOT_EXIST)
                    .with_detail("No Stripe customer is associated with this user.")
            }
            CreateBillingPortalSessionError::TemporarilyUnavailable { .. } => {
                ApiError::service_unavailable(BILLING_TEMPORARILY_UNAVAILABLE)
                    .with_detail("Billing is temporarily unavailable.")
            }
            CreateBillingPortalSessionError::ProviderRejected { .. }
            | CreateBillingPortalSessionError::ProviderInvalidResponse { .. } => {
                ApiError::internal_server_error(BILLING_PROVIDER_FAILURE)
                    .with_detail("Billing provider could not create a session.")
            }
            CreateBillingPortalSessionError::Internal { .. } => {
                ApiError::internal_server_error(BILLING_INTERNAL_ERROR)
                    .with_detail("Billing failed internally.")
            }
        }
    }
}

impl From<CreateBillingManagementSessionError> for ApiError {
    fn from(error: CreateBillingManagementSessionError) -> Self {
        match error {
            CreateBillingManagementSessionError::AuthenticatedActorRequired => {
                ApiError::unauthorized(INVALID_CREDENTIALS)
                    .with_header_field("Authorization")
                    .with_detail("Bearer token is required.")
            }
            CreateBillingManagementSessionError::Forbidden => {
                ApiError::forbidden(FORBIDDEN).with_detail("Operation is not permitted.")
            }
            CreateBillingManagementSessionError::UserNotFound => {
                ApiError::not_found(USER_NOT_FOUND).with_detail("User was not found.")
            }
            CreateBillingManagementSessionError::StripeCustomerDoesNotExist => {
                ApiError::unprocessable_content(STRIPE_CUSTOMER_DOES_NOT_EXIST)
                    .with_detail("No Stripe customer is associated with this user.")
            }
            CreateBillingManagementSessionError::StripeCustomerAssociationConflict => {
                ApiError::conflict(STRIPE_CUSTOMER_ASSOCIATION_CONFLICT)
                    .with_detail("Stripe customer association conflicts with current user state.")
            }
            CreateBillingManagementSessionError::ConcurrencyConflict => {
                ApiError::conflict(CONFLICT).with_detail("Billing state changed concurrently.")
            }
            CreateBillingManagementSessionError::TemporarilyUnavailable { .. } => {
                ApiError::service_unavailable(BILLING_TEMPORARILY_UNAVAILABLE)
                    .with_detail("Billing is temporarily unavailable.")
            }
            CreateBillingManagementSessionError::ProviderRejected { .. }
            | CreateBillingManagementSessionError::ProviderInvalidResponse { .. } => {
                ApiError::internal_server_error(BILLING_PROVIDER_FAILURE)
                    .with_detail("Billing provider could not create a session.")
            }
            CreateBillingManagementSessionError::Internal { .. } => {
                ApiError::internal_server_error(BILLING_INTERNAL_ERROR)
                    .with_detail("Billing failed internally.")
            }
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (
            self.status_code(),
            [(header::CONTENT_TYPE, "application/problem+json")],
            Json(self),
        )
            .into_response()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[tokio::test]
    async fn should_map_product_notification_read_failure_to_service_unavailable()
    -> Result<(), Box<dyn std::error::Error>> {
        let response = ApiError::from(GetProductError::ProductNotificationReadFailed {
            source: common::error::boxed::box_error(std::io::Error::other("dynamodb unavailable")),
        })
        .into_response();

        assert_eq!(StatusCode::SERVICE_UNAVAILABLE, response.status());
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX).await?;
        let body = serde_json::from_slice::<serde_json::Value>(&bytes)?;
        assert_eq!(PRODUCT_TEMPORARILY_UNAVAILABLE.to_string(), body["error"]);
        Ok(())
    }

    #[tokio::test]
    async fn should_map_temporary_jwks_failure_to_service_unavailable()
    -> Result<(), Box<dyn std::error::Error>> {
        let response = ApiError::from(AuthError::TemporarilyUnavailable).into_response();

        assert_eq!(StatusCode::SERVICE_UNAVAILABLE, response.status());
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX).await?;
        let body = serde_json::from_slice::<serde_json::Value>(&bytes)?;
        assert_eq!(AUTH_TEMPORARILY_UNAVAILABLE.to_string(), body["error"]);
        Ok(())
    }

    #[tokio::test]
    async fn should_render_problem_json_response() -> Result<(), Box<dyn std::error::Error>> {
        let response = ApiError::bad_request(INVALID_UUID)
            .with_path_field("shopId")
            .with_detail("Path parameter 'shopId' must be a UUID.")
            .into_response();

        assert_eq!(StatusCode::BAD_REQUEST, response.status());
        assert_eq!(
            "application/problem+json",
            response.headers()[header::CONTENT_TYPE]
        );
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX).await?;
        let body = serde_json::from_slice::<serde_json::Value>(&bytes)?;
        assert_eq!(
            json!({
                "status": 400,
                "title": "Bad Request",
                "error": INVALID_UUID.to_string(),
                "source": {"field": "shopId", "type": "PATH"},
                "detail": "Path parameter 'shopId' must be a UUID."
            }),
            body
        );
        Ok(())
    }
}
