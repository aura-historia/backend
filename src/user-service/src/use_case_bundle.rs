use crate::use_cases::{
    AuthenticateAccessTokenUseCase, ChangeUserRoleUseCase, ChangeUserTierUseCase,
    CheckUserAdminUseCase, CreateAccessTokenUseCase, CreateUserUseCase, DeleteAccessTokenUseCase,
    DeleteUserUseCase, FindUserByStripeCustomerIdUseCase, GetAccessTokenUseCase, GetUserUseCase,
    ListAccessTokensUseCase, SearchUsersUseCase, UpdateAccessTokenUseCase, UpdateUserUseCase,
};
use std::sync::Arc;

pub struct UserUseCases {
    pub create: Arc<dyn CreateUserUseCase>,
    pub update: Arc<dyn UpdateUserUseCase>,
    pub change_role: Arc<dyn ChangeUserRoleUseCase>,
    pub change_tier: Arc<dyn ChangeUserTierUseCase>,
    pub get: Arc<dyn GetUserUseCase>,
    pub check_admin: Arc<dyn CheckUserAdminUseCase>,
    pub search: Arc<dyn SearchUsersUseCase>,
    pub find_by_stripe_customer_id: Arc<dyn FindUserByStripeCustomerIdUseCase>,
    pub create_access_token: Arc<dyn CreateAccessTokenUseCase>,
    pub update_access_token: Arc<dyn UpdateAccessTokenUseCase>,
    pub delete_access_token: Arc<dyn DeleteAccessTokenUseCase>,
    pub delete: Arc<dyn DeleteUserUseCase>,
    pub get_access_token: Arc<dyn GetAccessTokenUseCase>,
    pub list_access_tokens: Arc<dyn ListAccessTokensUseCase>,
    pub authenticate_access_token: Arc<dyn AuthenticateAccessTokenUseCase>,
}

pub struct UserUseCasesInput {
    pub create: Arc<dyn CreateUserUseCase>,
    pub update: Arc<dyn UpdateUserUseCase>,
    pub change_role: Arc<dyn ChangeUserRoleUseCase>,
    pub change_tier: Arc<dyn ChangeUserTierUseCase>,
    pub get: Arc<dyn GetUserUseCase>,
    pub check_admin: Arc<dyn CheckUserAdminUseCase>,
    pub search: Arc<dyn SearchUsersUseCase>,
    pub find_by_stripe_customer_id: Arc<dyn FindUserByStripeCustomerIdUseCase>,
    pub create_access_token: Arc<dyn CreateAccessTokenUseCase>,
    pub update_access_token: Arc<dyn UpdateAccessTokenUseCase>,
    pub delete_access_token: Arc<dyn DeleteAccessTokenUseCase>,
    pub delete: Arc<dyn DeleteUserUseCase>,
    pub get_access_token: Arc<dyn GetAccessTokenUseCase>,
    pub list_access_tokens: Arc<dyn ListAccessTokensUseCase>,
    pub authenticate_access_token: Arc<dyn AuthenticateAccessTokenUseCase>,
}

impl UserUseCases {
    pub fn new(input: UserUseCasesInput) -> Self {
        Self {
            create: input.create,
            update: input.update,
            change_role: input.change_role,
            change_tier: input.change_tier,
            get: input.get,
            check_admin: input.check_admin,
            search: input.search,
            find_by_stripe_customer_id: input.find_by_stripe_customer_id,
            create_access_token: input.create_access_token,
            update_access_token: input.update_access_token,
            delete_access_token: input.delete_access_token,
            delete: input.delete,
            get_access_token: input.get_access_token,
            list_access_tokens: input.list_access_tokens,
            authenticate_access_token: input.authenticate_access_token,
        }
    }
}
