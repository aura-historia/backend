use aws_lambda_events::apigw::{ApiGatewayV2httpRequest, ApiGatewayV2httpResponse};
use common::{
    api::{
        api_gateway_v2_http_response_builder::ApiGatewayV2HttpResponseBuilder,
        error::ApiError,
        error_code::{BAD_BODY_VALUE, INTERNAL_SERVER_ERROR},
    },
    currency::domain::Currency,
    language::domain::Language,
    pagination::cursor::{
        Cursor,
        api::{JsonCursoredData, extract_json_cursor_query},
    },
    sort::api::extract_sort_query,
    user_id::{UserId, api::extract_user_id_request_context},
};
use lambda_runtime::LambdaEvent;
use user::{
    core::{sort_user_field::SortUserField, user_search::UserSearch},
    data::{
        get_user_data::GetUserAccountData, patch_admin_user_data::PatchAdminUserData,
        sort_user_field_data::SortUserFieldData, user_search_data::UserSearchData,
    },
    service::{command::UpdateUserCommand, user_service::UserService},
};

fn extract_user_id_path(
    path_params: &std::collections::HashMap<String, String>,
) -> Result<UserId, ApiError> {
    path_params
        .get("userId")
        .ok_or_else(|| {
            ApiError::internal_server_error(
                INTERNAL_SERVER_ERROR,
                "Missing path parameter 'userId' in AWS-Payload".into(),
            )
        })
        .map(String::as_str)
        .map(UserId::try_from)?
        .map_err(|err| ApiError::internal_server_error(INTERNAL_SERVER_ERROR, Box::new(err)))
}

async fn ensure_admin(
    event: &LambdaEvent<ApiGatewayV2httpRequest>,
    service: &(impl UserService + Sync),
) -> Result<(), ApiError> {
    let requester_id = extract_user_id_request_context(&event.payload.request_context)?;
    tracing::Span::current().record("userId", requester_id.to_string());
    service.check_admin(&requester_id).await?;
    Ok(())
}

pub async fn search(
    event: LambdaEvent<ApiGatewayV2httpRequest>,
    service: &(impl UserService + Sync),
) -> Result<ApiGatewayV2httpResponse, ApiError> {
    ensure_admin(&event, service).await?;
    let sort = extract_sort_query::<SortUserFieldData>(&event.payload.query_string_parameters)?
        .map(|sort_data| sort_data.map(SortUserField::from));
    let cursor =
        extract_json_cursor_query(&event.payload.query_string_parameters)?.unwrap_or(Cursor {
            size: 21,
            search_after: None,
        });
    let query = event
        .payload
        .raw_query_string
        .clone()
        .filter(|query| !query.is_empty())
        .unwrap_or_else(|| {
            let mut serializer = url::form_urlencoded::Serializer::new(String::new());
            for (key, value) in event.payload.query_string_parameters.iter() {
                serializer.append_pair(key, value);
            }
            serializer.finish()
        });
    let search_data: UserSearchData = serde_qs::from_str(&query).map_err(|err| {
        let err_msg = err.to_string();
        ApiError::bad_request(BAD_BODY_VALUE, Box::new(err)).with_detail(err_msg)
    })?;
    let search = UserSearch::from(search_data);
    let search_result = service
        .search_users(&search, &sort, &Some(cursor))
        .await?
        .map_item(GetUserAccountData::from);
    let search_result_data: JsonCursoredData<GetUserAccountData> =
        JsonCursoredData::from(search_result);

    Ok(ApiGatewayV2HttpResponseBuilder::json(200)
        .body_serde(search_result_data)?
        .build())
}

pub async fn get(
    event: LambdaEvent<ApiGatewayV2httpRequest>,
    service: &(impl UserService + Sync),
) -> Result<ApiGatewayV2httpResponse, ApiError> {
    ensure_admin(&event, service).await?;
    let user_id = extract_user_id_path(&event.payload.path_parameters)?;
    let user_data: GetUserAccountData = service.find_user(&user_id).await?.into();

    Ok(ApiGatewayV2HttpResponseBuilder::json(200)
        .last_modified(user_data.updated)
        .cache_control("no-store", None, None)
        .body_serde(user_data)?
        .build())
}

pub async fn patch(
    event: LambdaEvent<ApiGatewayV2httpRequest>,
    service: &(impl UserService + Sync),
) -> Result<ApiGatewayV2httpResponse, ApiError> {
    ensure_admin(&event, service).await?;
    let user_id = extract_user_id_path(&event.payload.path_parameters)?;
    let body = event
        .payload
        .body
        .filter(|str| !str.is_empty())
        .ok_or_else(|| {
            let err_msg = "Body cannot be empty";
            ApiError::bad_request(BAD_BODY_VALUE, err_msg.into()).with_detail(err_msg)
        })?;
    let patch_data: PatchAdminUserData = serde_json::from_str(&body).map_err(|err| {
        let err_msg = err.to_string();
        ApiError::bad_request(BAD_BODY_VALUE, Box::new(err)).with_detail(err_msg)
    })?;
    let cmd = UpdateUserCommand {
        first_name: patch_data.first_name,
        last_name: patch_data.last_name,
        language: patch_data.language.map(Language::from),
        currency: patch_data.currency.map(Currency::from),
        prohibited_content_consent: patch_data.prohibited_content_consent,
        tier: patch_data.tier.map(Into::into),
        role: patch_data.role.map(Into::into),
        stripe_customer_id: patch_data.stripe_customer_id,
    };
    let updated_user_data: GetUserAccountData = service.update_user(&user_id, cmd).await?.into();

    Ok(ApiGatewayV2HttpResponseBuilder::json(200)
        .last_modified(updated_user_data.updated)
        .body_serde(updated_user_data)?
        .build())
}

pub async fn delete(
    event: LambdaEvent<ApiGatewayV2httpRequest>,
    service: &(impl UserService + Sync),
) -> Result<ApiGatewayV2httpResponse, ApiError> {
    ensure_admin(&event, service).await?;
    let user_id = extract_user_id_path(&event.payload.path_parameters)?;
    service.delete_user(&user_id).await?;

    Ok(ApiGatewayV2HttpResponseBuilder::new(204).build())
}

#[cfg(test)]
mod tests {
    use super::*;
    use common::{pagination::cursor::CursoredResult, user_id::UserId};
    use fake::{Fake, Faker};
    use lambda_runtime::LambdaEvent;
    use test_api::ApiGatewayV2httpRequestProxy;
    use user::{
        core::user::User,
        service::user_service::{MockUserService, UserServiceError},
    };

    #[tokio::test]
    async fn should_search_users_when_admin_for_admin_endpoint() {
        let admin_id = UserId::new();
        let mut service = MockUserService::default();
        service
            .expect_check_admin()
            .return_once(move |_| Box::pin(async { Ok(()) }));
        service.expect_search_users().return_once(|search, _, _| {
            assert_eq!(Some("ada".try_into().unwrap()), search.query);
            Box::pin(async move {
                Ok(CursoredResult {
                    items: fake::vec![User; 2],
                    total: Some(2),
                    cursor: Default::default(),
                })
            })
        });
        let lambda_event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::GET)
                .route_key("GET /api/v1/users")
                .raw_query_string("query=ada".to_string())
                .jwt_claim("sub", admin_id)
                .build(),
            context: Default::default(),
        };

        let response = search(lambda_event, &service).await.unwrap();

        assert_eq!(200, response.status_code);
    }

    #[tokio::test]
    async fn should_forbid_search_users_when_not_admin_for_admin_endpoint() {
        let mut service = MockUserService::default();
        service.expect_check_admin().return_once(move |_| {
            Box::pin(async move { Err(UserServiceError::AdminRoleRequired) })
        });
        service.expect_search_users().never();
        let lambda_event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::GET)
                .route_key("GET /api/v1/users")
                .jwt_claim("sub", UserId::new())
                .build(),
            context: Default::default(),
        };

        let response = search(lambda_event, &service).await.unwrap_err();

        assert_eq!(403, response.status);
    }

    #[tokio::test]
    async fn should_get_user_when_admin_for_admin_endpoint() {
        let target_user: User = Faker.fake();
        let target_user_id = target_user.user_id;
        let mut service = MockUserService::default();
        service
            .expect_check_admin()
            .return_once(move |_| Box::pin(async { Ok(()) }));
        service.expect_find_user().return_once(move |user_id| {
            assert_eq!(&target_user_id, user_id);
            Box::pin(async move { Ok(target_user) })
        });
        let lambda_event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::GET)
                .route_key("GET /api/v1/users/{userId}")
                .path_parameter("userId", target_user_id.to_string())
                .jwt_claim("sub", UserId::new())
                .build(),
            context: Default::default(),
        };

        let response = get(lambda_event, &service).await.unwrap();

        assert_eq!(200, response.status_code);
    }

    #[tokio::test]
    async fn should_patch_user_when_admin_for_admin_endpoint() {
        let target_user: User = Faker.fake();
        let target_user_id = target_user.user_id;
        let mut service = MockUserService::default();
        service
            .expect_check_admin()
            .return_once(move |_| Box::pin(async { Ok(()) }));
        service
            .expect_update_user()
            .return_once(move |user_id, cmd| {
                assert_eq!(&target_user_id, user_id);
                assert!(cmd.tier.is_some());
                Box::pin(async move { Ok(target_user) })
            });
        let lambda_event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::PATCH)
                .route_key("PATCH /api/v1/users/{userId}")
                .path_parameter("userId", target_user_id.to_string())
                .jwt_claim("sub", UserId::new())
                .body_serde(&serde_json::json!({ "tier": "PRO" }))
                .build(),
            context: Default::default(),
        };

        let response = patch(lambda_event, &service).await.unwrap();

        assert_eq!(200, response.status_code);
    }

    #[tokio::test]
    async fn should_delete_user_when_admin_for_admin_endpoint() {
        let target_user_id = UserId::new();
        let mut service = MockUserService::default();
        service
            .expect_check_admin()
            .return_once(move |_| Box::pin(async { Ok(()) }));
        service.expect_delete_user().return_once(move |user_id| {
            assert_eq!(&target_user_id, user_id);
            Box::pin(async move { Ok(()) })
        });
        let lambda_event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::DELETE)
                .route_key("DELETE /api/v1/users/{userId}")
                .path_parameter("userId", target_user_id.to_string())
                .jwt_claim("sub", UserId::new())
                .build(),
            context: Default::default(),
        };

        let response = delete(lambda_event, &service).await.unwrap();

        assert_eq!(204, response.status_code);
    }
}
