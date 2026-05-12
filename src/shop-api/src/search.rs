use aws_lambda_events::apigw::{ApiGatewayV2httpRequest, ApiGatewayV2httpResponse};
use cognito::access_token_verifier_service::AccessTokenVerifierService;
use common::{
    api::{
        api_gateway_v2_http_response_builder::ApiGatewayV2HttpResponseBuilder, error::ApiError,
        error_code::BAD_BODY_VALUE,
    },
    pagination::cursor::{
        Cursor,
        api::{JsonCursoredData, extract_json_cursor_query},
    },
    sort::api::extract_sort_query,
};
use lambda_runtime::LambdaEvent;
use shop::core::shop_search::ShopSearch;
use shop::core::sort_shop_field::SortShopField;
use shop::data::{
    get_shop_data::GetShopData, shop_search_data::ShopSearchData,
    sort_shop_field_data::SortShopFieldData,
};
use shop::service::query_service::QueryShopService;

pub async fn handle(
    event: LambdaEvent<ApiGatewayV2httpRequest>,
    service: &impl QueryShopService,
    access_token_verifier_service: &(impl AccessTokenVerifierService + Sync),
) -> Result<ApiGatewayV2httpResponse, ApiError> {
    let user_id_opt = access_token_verifier_service
        .verify_extract_user_id(&event.payload.headers)
        .await?;
    if let Some(user_id) = user_id_opt {
        tracing::Span::current().record("userId", user_id.to_string());
    }

    let sort = extract_sort_query::<SortShopFieldData>(&event.payload.query_string_parameters)?
        .map(|sort_data| sort_data.map(SortShopField::from));
    let cursor =
        extract_json_cursor_query(&event.payload.query_string_parameters)?.unwrap_or(Cursor {
            size: 21,
            search_after: None,
        });
    let search_data: ShopSearchData = if event.payload.route_key.as_deref()
        == Some("GET /api/v1/shops")
    {
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
        serde_qs::from_str(&query).map_err(|err| {
            let err_msg = err.to_string();
            ApiError::bad_request(BAD_BODY_VALUE, Box::new(err)).with_detail(err_msg)
        })?
    } else {
        let body = event
            .payload
            .body
            .filter(|str| !str.is_empty())
            .ok_or_else(|| {
                let err_msg = "Body cannot be empty. If you want to search without any restrictions, supply the body '{}'.";
                ApiError::bad_request(BAD_BODY_VALUE, err_msg.into()).with_detail(err_msg)
            })?;
        serde_json::from_str(&body).map_err(|err| {
            let err_msg = err.to_string();
            ApiError::bad_request(BAD_BODY_VALUE, Box::new(err)).with_detail(err_msg)
        })?
    };

    let search = ShopSearch {
        shop_name_query: search_data.shop_name_query,
        shop_type_query: search_data
            .shop_type_query
            .into_iter()
            .map(Into::into)
            .collect(),
        partner_status_query: search_data
            .partner_status_query
            .into_iter()
            .map(Into::into)
            .collect(),
        countries: search_data.countries.into_iter().collect(),
        continents: search_data
            .continents
            .into_iter()
            .map(shop::core::continent::Continent::from)
            .collect(),
        created: search_data.created,
        updated: search_data.updated,
    };
    let search_result = service
        .search_shops(&search, &sort, &Some(cursor))
        .await?
        .map_item(GetShopData::from);
    let search_result_data: JsonCursoredData<GetShopData> = JsonCursoredData::from(search_result);

    let response_builder = ApiGatewayV2HttpResponseBuilder::json(200);
    let response_builder = match (event.payload.route_key.as_deref(), user_id_opt) {
        (Some("GET /api/v1/shops"), Some(_)) => {
            response_builder.cache_control("no-store", None, None)
        }
        (Some("GET /api/v1/shops"), None) => {
            response_builder.cache_control("public", Some(600), Some(3600))
        }
        _ => response_builder,
    };

    Ok(response_builder.body_serde(search_result_data)?.build())
}

#[cfg(test)]
mod tests {
    use crate::handle;
    use cognito::access_token_verifier_service::MockAccessTokenVerifierService;
    use common::pagination::cursor::{Cursor, CursoredResult};
    use fake::Fake;
    use fake::Faker;
    use lambda_runtime::LambdaEvent;
    use shop::core::shop::Shop;
    use shop::data::shop_search_data::ShopSearchData;
    use shop::service::command_service::MockCommandShopService;
    use shop::service::get_service::MockGetShopService;
    use shop::service::query_service::MockQueryShopService;
    use test_api::ApiGatewayV2httpRequestProxy;
    use user::service::user_service::MockUserService;

    #[tokio::test]
    #[rstest::rstest]
    #[case(Some("name"), Some("asc"))]
    #[case(Some("created"), Some("desc"))]
    #[case(None, None)]
    #[case(None, None)]
    #[trace]
    async fn should_handle_request(#[case] sort: Option<&str>, #[case] order: Option<&str>) {
        let lambda_event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::POST)
                .route_key("POST /api/v1/shops/search")
                .try_query_string_parameter("sort", sort)
                .try_query_string_parameter("order", order)
                .body_serde(&Faker.fake::<ShopSearchData>())
                .build(),
            context: Default::default(),
        };

        let mut service = MockQueryShopService::default();
        service
            .expect_search_shops()
            .return_once(move |_, _, cursor| {
                let count = cursor.clone().map(|cursor| cursor.size).unwrap_or(20) as usize;
                let search_result = CursoredResult {
                    items: fake::vec![Shop; count],
                    total: Some(789),
                    cursor: Cursor {
                        size: count as u64,
                        search_after: None,
                    },
                };
                Box::pin(async move { Ok(search_result) })
            });
        let mut access_token_verifier_service = MockAccessTokenVerifierService::default();
        access_token_verifier_service
            .expect_verify_extract_user_id()
            .return_once(|_| Box::pin(async { Ok(None) }));
        let response = handle(
            lambda_event,
            &MockGetShopService::default(),
            &service,
            &MockCommandShopService::default(),
            &MockUserService::default(),
            &access_token_verifier_service,
        )
        .await
        .unwrap();

        assert_eq!(200, response.status_code);
    }

    #[tokio::test]
    async fn should_handle_get_request_with_partner_status_filter() {
        use shop::core::partner_status::ShopPartnerStatus;
        use std::collections::HashSet;

        let raw_query = "shopNameQuery=weitze&partnerStatus=PARTNERED";
        let lambda_event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::GET)
                .route_key("GET /api/v1/shops")
                .raw_query_string(raw_query.to_string())
                .build(),
            context: Default::default(),
        };

        let mut service = MockQueryShopService::default();
        service.expect_search_shops().return_once(|search, _, _| {
            let expected: HashSet<ShopPartnerStatus> =
                HashSet::from_iter([ShopPartnerStatus::Partnered]);
            assert_eq!(
                expected,
                HashSet::from_iter(search.partner_status_query.iter().copied())
            );
            Box::pin(async move {
                Ok(CursoredResult {
                    items: vec![],
                    total: Some(0),
                    cursor: Cursor {
                        size: 21,
                        search_after: None,
                    },
                })
            })
        });
        let mut access_token_verifier_service = MockAccessTokenVerifierService::default();
        access_token_verifier_service
            .expect_verify_extract_user_id()
            .return_once(|_| Box::pin(async { Ok(None) }));
        let response = handle(
            lambda_event,
            &MockGetShopService::default(),
            &service,
            &MockCommandShopService::default(),
            &MockUserService::default(),
            &access_token_verifier_service,
        )
        .await
        .unwrap();

        assert_eq!(200, response.status_code);
    }

    #[tokio::test]
    async fn should_handle_get_simple_search_with_query_params_when_unauthenticated() {
        let lambda_event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::GET)
                .route_key("GET /api/v1/shops")
                .query_string_parameter("shopNameQuery", "House")
                .query_string_parameter("sort", "name")
                .query_string_parameter("order", "asc")
                .build(),
            context: Default::default(),
        };

        let mut service = MockQueryShopService::default();
        service.expect_search_shops().return_once(|search, _, _| {
            assert_eq!(Some("House".try_into().unwrap()), search.shop_name_query);
            Box::pin(async move {
                Ok(CursoredResult {
                    items: vec![],
                    total: Some(0),
                    cursor: Cursor {
                        size: 21,
                        search_after: None,
                    },
                })
            })
        });
        let mut access_token_verifier_service = MockAccessTokenVerifierService::default();
        access_token_verifier_service
            .expect_verify_extract_user_id()
            .return_once(|_| Box::pin(async { Ok(None) }));
        let response = handle(
            lambda_event,
            &MockGetShopService::default(),
            &service,
            &MockCommandShopService::default(),
            &MockUserService::default(),
            &access_token_verifier_service,
        )
        .await
        .unwrap();

        assert_eq!(200, response.status_code);
        assert_eq!(
            "public, max-age=600, s-maxage=3600",
            response
                .headers
                .get(http::header::CACHE_CONTROL)
                .unwrap()
                .to_str()
                .unwrap()
        );
    }

    #[tokio::test]
    async fn should_set_cache_control_to_no_store_when_authenticated_for_get_shops() {
        use common::user_id::UserId;

        let lambda_event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::GET)
                .route_key("GET /api/v1/shops")
                .query_string_parameter("shopNameQuery", "House")
                .build(),
            context: Default::default(),
        };

        let mut service = MockQueryShopService::default();
        service.expect_search_shops().return_once(|_, _, _| {
            Box::pin(async move {
                Ok(CursoredResult {
                    items: vec![],
                    total: Some(0),
                    cursor: Cursor {
                        size: 21,
                        search_after: None,
                    },
                })
            })
        });
        let user_id = UserId::new();
        let mut access_token_verifier_service = MockAccessTokenVerifierService::default();
        access_token_verifier_service
            .expect_verify_extract_user_id()
            .return_once(move |_| Box::pin(async move { Ok(Some(user_id)) }));
        let response = handle(
            lambda_event,
            &MockGetShopService::default(),
            &service,
            &MockCommandShopService::default(),
            &MockUserService::default(),
            &access_token_verifier_service,
        )
        .await
        .unwrap();

        assert_eq!(200, response.status_code);
        assert_eq!(
            "no-store",
            response
                .headers
                .get(http::header::CACHE_CONTROL)
                .unwrap()
                .to_str()
                .unwrap()
        );
    }

    #[tokio::test]
    async fn should_allow_empty_shop_search() {
        let lambda_event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::POST)
                .route_key("POST /api/v1/shops/search")
                .body_serde(&ShopSearchData::default())
                .build(),
            context: Default::default(),
        };

        let mut service = MockQueryShopService::default();
        service.expect_search_shops().return_once(|_, _, cursor| {
            let count = cursor.clone().map(|cursor| cursor.size).unwrap_or(20) as usize;
            let search_result = CursoredResult {
                items: fake::vec![Shop; count],
                total: Some(789),
                cursor: Cursor {
                    size: count as u64,
                    search_after: None,
                },
            };
            Box::pin(async move { Ok(search_result) })
        });
        let mut access_token_verifier_service = MockAccessTokenVerifierService::default();
        access_token_verifier_service
            .expect_verify_extract_user_id()
            .return_once(|_| Box::pin(async { Ok(None) }));
        let response = handle(
            lambda_event,
            &MockGetShopService::default(),
            &service,
            &MockCommandShopService::default(),
            &MockUserService::default(),
            &access_token_verifier_service,
        )
        .await
        .unwrap();

        assert_eq!(200, response.status_code);
    }
}
