use aws_lambda_events::apigw::{ApiGatewayV2httpRequest, ApiGatewayV2httpResponse};
use cognito::access_token_verifier_service::AccessTokenVerifierService;
use common::{
    api::{
        api_gateway_v2_http_response_builder::ApiGatewayV2HttpResponseBuilder,
        error::{ApiError, log_api_error},
    },
    currency::data::api::extract_currency_query,
    language::{data::api::extract_languages_header, domain::Language},
    personalized::{Personalized, api::PersonalizedData},
    shop_id::api::extract_shop_id_path,
    shops_product_id::api::extract_shops_product_id_path,
};
use lambda_runtime::LambdaEvent;
use product::core::user_state::ProductUserState;
use product::{
    data::get_data::GetProductData, service::personalization_service::ProductPersonalizationService,
};
use product::{
    data::user_state_data::ProductUserStateData, service::semantic_service::SemanticSearchService,
};

#[tracing::instrument(
    skip(event, semantic_search_service, access_token_verifier_service, product_personalization_service),
    fields(
        requestId = %event.context.request_id,
        method = event.payload.request_context.http.method.as_str(),
        path = &event.payload.raw_path.as_deref().unwrap_or("NULL"),
        query = &event.payload.raw_query_string.as_deref().unwrap_or("NULL"),
        body = &event.payload.body.as_deref().unwrap_or("NULL"),
        ip = &event.payload.request_context.http.source_ip.as_deref().unwrap_or("NULL"),
        userAgent = &event.payload.request_context.http.user_agent.as_deref().unwrap_or("NULL"),
        userId = tracing::field::Empty,
    )
)]
pub async fn handler(
    event: LambdaEvent<ApiGatewayV2httpRequest>,
    semantic_search_service: &impl SemanticSearchService,
    access_token_verifier_service: &(impl AccessTokenVerifierService + Sync),
    product_personalization_service: &impl ProductPersonalizationService,
) -> Result<ApiGatewayV2httpResponse, lambda_runtime::Error> {
    match handle(
        event,
        semantic_search_service,
        access_token_verifier_service,
        product_personalization_service,
    )
    .await
    {
        Ok(response) => Ok(response),
        Err(err) => {
            log_api_error(&err);
            Ok(ApiGatewayV2httpResponse::from(err))
        }
    }
}

// GET /api/v1/products/{shopId}/{shopsProductId}/similar
pub async fn handle(
    event: LambdaEvent<ApiGatewayV2httpRequest>,
    semantic_search_service: &impl SemanticSearchService,
    access_token_verifier_service: &(impl AccessTokenVerifierService + Sync),
    product_personalization_service: &impl ProductPersonalizationService,
) -> Result<ApiGatewayV2httpResponse, ApiError> {
    let user_id_opt = access_token_verifier_service
        .verify_extract_user_id(&event.payload.headers)
        .await?;
    if let Some(user_id) = user_id_opt {
        tracing::Span::current().record("userId", user_id.to_string());
    }

    let languages = extract_languages_header(&event.payload.headers)?
        .into_iter()
        .map(Language::from)
        .collect::<Vec<_>>();
    let currency = extract_currency_query(&event.payload.query_string_parameters)?;
    let shop_id = extract_shop_id_path(&event.payload.path_parameters)?;
    let shops_product_id = extract_shops_product_id_path(&event.payload.path_parameters)?;

    let localized_similar_products_opt = semantic_search_service
        .similar_products(&shop_id, &shops_product_id, &languages, &currency.into())
        .await?;

    match localized_similar_products_opt {
        None => {
            let location = match event.payload.request_context.domain_name {
                None => None,
                Some(domain_name) => event.payload.request_context.stage.map(|stage_name| format!(
                        "https://{domain_name}/{stage_name}/api/v1/products/{shop_id}/{shops_product_id}/similar",
                    )),
            };
            Ok(ApiGatewayV2HttpResponseBuilder::json(202)
                .try_location(location.as_deref())
                .build())
        }
        Some(localized_similar_items) => {
            let personalized_similar_items = match user_id_opt {
                None => localized_similar_items
                    .into_iter()
                    .map(|item| Personalized {
                        item,
                        user_state: None,
                    })
                    .collect(),
                Some(user_id) => product_personalization_service
                    .personalize_all_watchlist(&user_id, localized_similar_items)
                    .await?
                    .into_iter()
                    .map(|personalized_item| Personalized {
                        item: personalized_item.item,
                        user_state: personalized_item
                            .user_state
                            .map(|watchlist| ProductUserState { watchlist }),
                    })
                    .collect::<Vec<_>>(),
            };

            let similar_products_data: Vec<PersonalizedData<GetProductData, ProductUserStateData>> =
                personalized_similar_items
                    .into_iter()
                    .map(PersonalizedData::from)
                    .collect();

            Ok(ApiGatewayV2HttpResponseBuilder::json(200)
                .body_serde(similar_products_data)?
                .build())
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::handler;
    use cognito::access_token_verifier_service::MockAccessTokenVerifierService;
    use common::shop_id::ShopId;
    use common::shops_product_id::ShopsProductId;
    use fake::Fake;
    use fake::Faker;
    use http::header::LOCATION;
    use lambda_runtime::LambdaEvent;
    use product::service::personalization_service::MockProductPersonalizationService;
    use product::service::semantic_service::MockSemanticSearchService;
    use product::service::semantic_service::SemanticSearchProductsError;
    use test_api::{ApiGatewayV2httpRequestProxy, extract_apigw_response_json_body};

    #[tokio::test]
    async fn should_200_when_similar_products_have_been_computed_and_empty() {
        let mut cognito_service = MockAccessTokenVerifierService::default();
        cognito_service
            .expect_verify_extract_user_id()
            .return_once(|_| Box::pin(async { Ok(None) }));
        let product_personalization_service = MockProductPersonalizationService::default();
        let mut semantic_search_service = MockSemanticSearchService::default();
        semantic_search_service
            .expect_similar_products()
            .return_once(|_, _, _, _| Box::pin(async { Ok(Some(vec![])) }));

        let lambda_event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::GET)
                .path_parameter("shopId", ShopId::new())
                .path_parameter("shopsProductId", ShopsProductId::new())
                .build(),
            context: Default::default(),
        };

        let response = handler(
            lambda_event,
            &semantic_search_service,
            &cognito_service,
            &product_personalization_service,
        )
        .await
        .unwrap();
        assert_eq!(200, response.status_code);
    }

    #[tokio::test]
    async fn should_200_when_similar_products_have_been_computed_and_not_empty() {
        let mut cognito_service = MockAccessTokenVerifierService::default();
        cognito_service
            .expect_verify_extract_user_id()
            .return_once(|_| Box::pin(async { Ok(None) }));
        let product_personalization_service = MockProductPersonalizationService::default();
        let mut semantic_search_service = MockSemanticSearchService::default();
        semantic_search_service
            .expect_similar_products()
            .return_once(|_, _, _, _| Box::pin(async { Ok(Some(Faker.fake())) }));

        let lambda_event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::GET)
                .path_parameter("shopId", ShopId::new())
                .path_parameter("shopsProductId", ShopsProductId::new())
                .build(),
            context: Default::default(),
        };

        let response = handler(
            lambda_event,
            &semantic_search_service,
            &cognito_service,
            &product_personalization_service,
        )
        .await
        .unwrap();
        assert_eq!(200, response.status_code);
    }

    #[tokio::test]
    async fn should_202_when_similar_products_have_not_been_computed() {
        let mut cognito_service = MockAccessTokenVerifierService::default();
        cognito_service
            .expect_verify_extract_user_id()
            .return_once(|_| Box::pin(async { Ok(None) }));
        let product_personalization_service = MockProductPersonalizationService::default();
        let mut semantic_search_service = MockSemanticSearchService::default();
        semantic_search_service
            .expect_similar_products()
            .return_once(|_, _, _, _| Box::pin(async { Ok(None) }));

        let shop_id = ShopId::new();
        let shops_product_id = ShopsProductId::new();
        let lambda_event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::GET)
                .path_parameter("shopId", shop_id)
                .path_parameter("shopsProductId", shops_product_id.clone())
                .domain_name("my.domain.com")
                .stage("prod")
                .build(),
            context: Default::default(),
        };

        let response = handler(
            lambda_event,
            &semantic_search_service,
            &cognito_service,
            &product_personalization_service,
        )
        .await
        .unwrap();
        assert_eq!(202, response.status_code);
        assert_eq!(
            format!(
                "https://my.domain.com/prod/api/v1/products/{shop_id}/{shops_product_id}/similar"
            ),
            response.headers.get(LOCATION).unwrap().to_str().unwrap()
        )
    }

    #[tokio::test]
    async fn should_400_when_path_param_shop_id_is_missing() {
        let mut cognito_service = MockAccessTokenVerifierService::default();
        cognito_service
            .expect_verify_extract_user_id()
            .return_once(|_| Box::pin(async { Ok(None) }));
        let product_personalization_service = MockProductPersonalizationService::default();
        let semantic_search_service = MockSemanticSearchService::default();
        let lambda_event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::GET)
                .path_parameter("shopsProductId", ShopsProductId::new())
                .build(),
            context: Default::default(),
        };

        let response = handler(
            lambda_event,
            &semantic_search_service,
            &cognito_service,
            &product_personalization_service,
        )
        .await
        .unwrap();
        assert_eq!(400, response.status_code);
        let json = extract_apigw_response_json_body!(response);
        assert_eq!(400, json["status"]);
        assert_eq!("shopId", json["source"]["field"]);
    }

    #[tokio::test]
    async fn should_400_when_path_param_shops_product_id_is_missing() {
        let mut cognito_service = MockAccessTokenVerifierService::default();
        cognito_service
            .expect_verify_extract_user_id()
            .return_once(|_| Box::pin(async { Ok(None) }));
        let product_personalization_service = MockProductPersonalizationService::default();
        let semantic_search_service = MockSemanticSearchService::default();
        let lambda_event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::GET)
                .path_parameter("shopId", ShopId::new())
                .build(),
            context: Default::default(),
        };

        let response = handler(
            lambda_event,
            &semantic_search_service,
            &cognito_service,
            &product_personalization_service,
        )
        .await
        .unwrap();
        assert_eq!(400, response.status_code);
        let json = extract_apigw_response_json_body!(response);
        assert_eq!(400, json["status"]);
        assert_eq!("shopsProductId", json["source"]["field"]);
    }

    #[tokio::test]
    async fn should_404_when_product_does_not_exist() {
        let shop_id = ShopId::new();
        let shops_product_id = ShopsProductId::new();
        let lambda_event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::GET)
                .path_parameter("shopId", shop_id)
                .path_parameter("shopsProductId", shops_product_id)
                .build(),
            context: Default::default(),
        };

        let mut cognito_service = MockAccessTokenVerifierService::default();
        cognito_service
            .expect_verify_extract_user_id()
            .return_once(|_| Box::pin(async { Ok(None) }));
        let product_personalization_service = MockProductPersonalizationService::default();
        let mut semantic_search_service = MockSemanticSearchService::default();
        semantic_search_service
            .expect_similar_products()
            .return_once(move |shop_id, shops_product_id, _, _| {
                let shop_id = *shop_id;
                let shops_product_id = shops_product_id.clone();
                Box::pin(async move {
                    Err(SemanticSearchProductsError::ProductNotFound(
                        shop_id,
                        shops_product_id,
                    ))
                })
            });

        let response = handler(
            lambda_event,
            &semantic_search_service,
            &cognito_service,
            &product_personalization_service,
        )
        .await
        .unwrap();
        assert_eq!(404, response.status_code);
        let json = extract_apigw_response_json_body!(response);
        assert_eq!(404, json["status"]);
    }
}
