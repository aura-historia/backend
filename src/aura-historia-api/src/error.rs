use crate::auth::AuthError;
use axum::Json;
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde::Serialize;
use shop_partner_service::use_cases::{
    AdminDecidePartnerShopApplicationError, AdminGetPartnerShopApplicationError,
    AdminListPartnerShopApplicationsError, AdminUpdatePartnerShopApplicationError,
    CreatePartnerShopApplicationError, DeletePartnerShopApplicationError,
    GetPartnerShopApplicationError, ListPartnerShopApplicationsError,
    UpdatePartnerShopApplicationError,
};
use shop_service::use_cases::commands::create_shop::CreateShopError;
use shop_service::use_cases::commands::update_shop::UpdateShopError;
use shop_service::use_cases::queries::check_user_partner_shop::CheckUserPartnerShopError;
use shop_service::use_cases::queries::get_shop::GetShopError;
use shop_service::use_cases::queries::list_user_partner_shops::ListUserPartnerShopsError;
use shop_service::use_cases::queries::search_shops::SearchShopsError;
use std::error::Error;
use std::fmt::{Display, Formatter};
use user_service::use_cases::commands::create_access_token::CreateAccessTokenError;
use user_service::use_cases::commands::delete_access_token::DeleteAccessTokenError;
use user_service::use_cases::commands::delete_user::DeleteUserError;
use user_service::use_cases::commands::update_access_token::UpdateAccessTokenError;
use user_service::use_cases::commands::update_user::UpdateUserError;
use user_service::use_cases::queries::check_user_admin::CheckUserAdminError;
use user_service::use_cases::queries::get_access_token::GetAccessTokenError;
use user_service::use_cases::queries::get_user::GetUserError;
use user_service::use_cases::queries::list_access_tokens::ListAccessTokensError;
use user_service::use_cases::queries::search_users::SearchUsersError;
use watchlist_service::use_cases::{
    DeleteWatchlistProductError, ListWatchlistError, UpdateWatchlistProductError, WatchProductError,
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
pub(crate) const BAD_ORDER_VALUE: ApiErrorCode = ApiErrorCode("BAD_ORDER_VALUE");
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
pub(crate) const USER_INTERNAL_ERROR: ApiErrorCode = ApiErrorCode("USER_INTERNAL_ERROR");
pub(crate) const USER_NOT_FOUND: ApiErrorCode = ApiErrorCode("USER_NOT_FOUND");
pub(crate) const USER_TEMPORARILY_UNAVAILABLE: ApiErrorCode =
    ApiErrorCode("USER_TEMPORARILY_UNAVAILABLE");
pub(crate) const WATCHLIST_ENTRY_NOT_FOUND: ApiErrorCode =
    ApiErrorCode("WATCHLIST_ENTRY_NOT_FOUND");
pub(crate) const WATCHLIST_INTERNAL_ERROR: ApiErrorCode = ApiErrorCode("WATCHLIST_INTERNAL_ERROR");
pub(crate) const WATCHLIST_TEMPORARILY_UNAVAILABLE: ApiErrorCode =
    ApiErrorCode("WATCHLIST_TEMPORARILY_UNAVAILABLE");
pub(crate) const PARTNER_SHOP_APPLICATION_NOT_FOUND: ApiErrorCode =
    ApiErrorCode("PARTNER_SHOP_APPLICATION_NOT_FOUND");
pub(crate) const PARTNER_SHOP_APPLICATION_INTERNAL_ERROR: ApiErrorCode =
    ApiErrorCode("PARTNER_SHOP_APPLICATION_INTERNAL_ERROR");
pub(crate) const PARTNER_SHOP_APPLICATION_TEMPORARILY_UNAVAILABLE: ApiErrorCode =
    ApiErrorCode("PARTNER_SHOP_APPLICATION_TEMPORARILY_UNAVAILABLE");

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

impl From<CheckUserAdminError> for ApiError {
    fn from(error: CheckUserAdminError) -> Self {
        match error {
            CheckUserAdminError::UserNotFound => {
                ApiError::not_found(USER_NOT_FOUND).with_detail("User was not found.")
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

impl From<GetUserError> for ApiError {
    fn from(error: GetUserError) -> Self {
        match error {
            GetUserError::NotFound => {
                ApiError::not_found(USER_NOT_FOUND).with_detail("User was not found.")
            }
            GetUserError::TemporarilyUnavailable { .. }
            | GetUserError::BeginTransactionFailed
            | GetUserError::CommitTransactionFailed => {
                ApiError::service_unavailable(USER_TEMPORARILY_UNAVAILABLE)
                    .with_detail("User details are temporarily unavailable.")
            }
            GetUserError::InvalidReadModel { .. } | GetUserError::Internal { .. } => {
                ApiError::internal_server_error(USER_INTERNAL_ERROR)
                    .with_detail("User details failed internally.")
            }
        }
    }
}

impl From<SearchUsersError> for ApiError {
    fn from(error: SearchUsersError) -> Self {
        match error {
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

impl From<UpdateUserError> for ApiError {
    fn from(error: UpdateUserError) -> Self {
        match error {
            UpdateUserError::AuthenticatedActorRequired => {
                ApiError::unauthorized(INVALID_CREDENTIALS)
                    .with_header_field("Authorization")
                    .with_detail("Bearer token is required.")
            }
            UpdateUserError::Forbidden => {
                ApiError::forbidden(FORBIDDEN).with_detail("Operation is not permitted.")
            }
            UpdateUserError::UserNotFound => {
                ApiError::not_found(USER_NOT_FOUND).with_detail("User was not found.")
            }
            UpdateUserError::ConcurrencyConflict
            | UpdateUserError::EmailConflict { .. }
            | UpdateUserError::StripeCustomerConflict { .. } => ApiError::conflict(CONFLICT)
                .with_detail("User update conflicts with current state."),
            UpdateUserError::EmailRequired
            | UpdateUserError::TierRequired
            | UpdateUserError::RoleRequired
            | UpdateUserError::InvalidUserState { .. } => {
                ApiError::bad_request(BAD_BODY_VALUE).with_detail("User update is invalid.")
            }
            UpdateUserError::TemporarilyUnavailable { .. }
            | UpdateUserError::BeginTransactionFailed
            | UpdateUserError::CommitTransactionFailed => {
                ApiError::service_unavailable(USER_TEMPORARILY_UNAVAILABLE)
                    .with_detail("User could not be updated right now.")
            }
            UpdateUserError::InvalidPersistedState { .. } | UpdateUserError::Internal { .. } => {
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
                    .with_detail("User could not be deleted right now.")
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
            WatchProductError::TemporarilyUnavailable
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
            UpdateWatchlistProductError::TemporarilyUnavailable
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
impl From<DeleteWatchlistProductError> for ApiError {
    fn from(error: DeleteWatchlistProductError) -> Self {
        match error {
            DeleteWatchlistProductError::AuthenticatedActorRequired => {
                ApiError::unauthorized(INVALID_CREDENTIALS)
                    .with_header_field("Authorization")
                    .with_detail("Bearer token is required.")
            }
            DeleteWatchlistProductError::Forbidden => {
                ApiError::forbidden(FORBIDDEN).with_detail("Operation is not permitted.")
            }
            DeleteWatchlistProductError::NotFound => ApiError::not_found(WATCHLIST_ENTRY_NOT_FOUND)
                .with_detail("Watchlist entry was not found."),
            DeleteWatchlistProductError::TemporarilyUnavailable
            | DeleteWatchlistProductError::BeginTransactionFailed
            | DeleteWatchlistProductError::CommitTransactionFailed => {
                ApiError::service_unavailable(WATCHLIST_TEMPORARILY_UNAVAILABLE)
                    .with_detail("Watchlist is temporarily unavailable.")
            }
            DeleteWatchlistProductError::InvalidPersistedState => {
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
impl_partner_shop_application_error!(
    UpdatePartnerShopApplicationError,
    NotFound,
    ConcurrencyConflict
);
impl_partner_shop_application_error!(
    DeletePartnerShopApplicationError,
    NotFound,
    ConcurrencyConflict
);
impl_partner_shop_application_error!(
    AdminGetPartnerShopApplicationError,
    NotFound,
    ConcurrencyConflict
);
impl_partner_shop_application_error!(
    AdminUpdatePartnerShopApplicationError,
    NotFound,
    ConcurrencyConflict
);
impl_partner_shop_application_error!(
    AdminDecidePartnerShopApplicationError,
    NotFound,
    ConcurrencyConflict
);

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
