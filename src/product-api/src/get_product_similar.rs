use aws_lambda_events::apigw::{ApiGatewayV2httpRequest, ApiGatewayV2httpResponse};
use cognito::access_token_verifier_service::AccessTokenVerifierService;
use common::{
    api::{api_gateway_v2_http_response_builder::ApiGatewayV2HttpResponseBuilder, error::ApiError},
    currency::data::api::extract_currency_query,
    language::{data::api::extract_language_query, domain::Language},
    personalized::api::PersonalizedData,
    shop_id::api::extract_shop_id_path,
    shops_product_id::api::extract_shops_product_id_path,
};
use lambda_runtime::LambdaEvent;
use product::data::get_summary_data::GetProductSummaryData;
use product::{
    data::user_state_data::ProductUserStateData, service::semantic_service::SemanticSearchService,
};
use product_personalization::service::ProductPersonalizationService;

pub async fn handle(
    event: LambdaEvent<ApiGatewayV2httpRequest>,
    semantic_search_service: &impl SemanticSearchService,
    access_token_verifier_service: &(impl AccessTokenVerifierService + Sync),
    product_personalization_service: &impl ProductPersonalizationService,
) -> Result<ApiGatewayV2httpResponse, ApiError> {
    let user_id_opt = access_token_verifier_service
        .verify_extract_user_id(&event.payload.headers)
        .await
        .map_err(crate::map_access_token_error)?;
    if let Some(user_id) = user_id_opt {
        tracing::Span::current().record("userId", user_id.to_string());
    }

    let languages = vec![Language::from(extract_language_query(
        &event.payload.query_string_parameters,
    )?)];
    let currency = extract_currency_query(&event.payload.query_string_parameters)?;
    let shop_id = extract_shop_id_path(&event.payload.path_parameters)?;
    let shops_product_id = extract_shops_product_id_path(&event.payload.path_parameters)?;

    let localized_similar_products_opt = semantic_search_service
        .similar_products(&shop_id, &shops_product_id, &languages, &currency.into())
        .await?;

    match localized_similar_products_opt {
        None => Ok(ApiGatewayV2HttpResponseBuilder::json(202)
            .location(
                &format!("shops/{shop_id}/products/{shops_product_id}/similar"),
                &event.payload.request_context,
            )
            .cache_control("public", Some(300), Some(900))
            .build()),
        Some(localized_similar_products) => {
            let similar_products_data: Vec<
                PersonalizedData<GetProductSummaryData, ProductUserStateData>,
            > = match user_id_opt {
                None => localized_similar_products
                    .into_iter()
                    .map(|item| PersonalizedData {
                        item: GetProductSummaryData::from_view(item, false),
                        user_state: None,
                    })
                    .collect(),
                Some(user_id) => product_personalization_service
                    .personalize_all(&user_id, localized_similar_products)
                    .await?
                    .into_iter()
                    .map(|personalized| {
                        let consent = personalized
                            .user_state
                            .clone()
                            .map(|s| s.prohibited_content.consent)
                            .unwrap_or(false);
                        PersonalizedData {
                            item: GetProductSummaryData::from_view(personalized.item, consent),
                            user_state: personalized.user_state.map(Into::into),
                        }
                    })
                    .collect(),
            };

            let cache_control_directive = if user_id_opt.is_some() {
                "no-store"
            } else {
                "public"
            };
            let cache_control_max_age = if user_id_opt.is_some() {
                None
            } else {
                Some(180)
            };
            let cache_control_x_max_age = if user_id_opt.is_some() {
                None
            } else {
                Some(900)
            };

            Ok(ApiGatewayV2HttpResponseBuilder::json(200)
                .cache_control(
                    cache_control_directive,
                    cache_control_max_age,
                    cache_control_x_max_age,
                )
                .body_serde(similar_products_data)?
                .build())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::handle;
    use cognito::access_token_verifier_service::MockAccessTokenVerifierService;
    use common::personalized::Personalized;
    use common::shop_id::ShopId;
    use common::shops_product_id::ShopsProductId;
    use common::user_id::UserId;
    use fake::Fake;
    use fake::Faker;
    use http::header::CACHE_CONTROL;
    use lambda_runtime::LambdaEvent;
    use product::service::semantic_service::MockSemanticSearchService;
    use product::service::semantic_service::SemanticSearchProductsError;
    use product_personalization::service::MockProductPersonalizationService;
    use test_api::ApiGatewayV2httpRequestProxy;

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

        let response = handle(
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

        let response = handle(
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
                .stage("prod")
                .build(),
            context: Default::default(),
        };

        let response = handle(
            lambda_event,
            &semantic_search_service,
            &cognito_service,
            &product_personalization_service,
        )
        .await
        .unwrap();
        assert_eq!(202, response.status_code);
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

        let actual = handle(
            lambda_event,
            &semantic_search_service,
            &cognito_service,
            &product_personalization_service,
        )
        .await
        .unwrap_err();
        assert_eq!(400, actual.status);
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

        let actual = handle(
            lambda_event,
            &semantic_search_service,
            &cognito_service,
            &product_personalization_service,
        )
        .await
        .unwrap_err();
        assert_eq!(400, actual.status);
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

        let actual = handle(
            lambda_event,
            &semantic_search_service,
            &cognito_service,
            &product_personalization_service,
        )
        .await
        .unwrap_err();
        assert_eq!(404, actual.status);
    }

    #[tokio::test]
    async fn should_set_cache_control_to_no_store_when_user_is_authenticated_for_get_product_similar()
     {
        let mut cognito_service = MockAccessTokenVerifierService::default();
        cognito_service
            .expect_verify_extract_user_id()
            .return_once(|_| Box::pin(async { Ok(Some(UserId::new())) }));
        let mut product_personalization_service = MockProductPersonalizationService::default();
        product_personalization_service
            .expect_personalize_all()
            .return_once(|_, products| {
                let personalized: Vec<_> = products
                    .into_iter()
                    .map(|item| Personalized {
                        item,
                        user_state: Some(Default::default()),
                    })
                    .collect();
                Box::pin(async move { Ok(personalized) })
            });
        let mut semantic_search_service = MockSemanticSearchService::default();
        semantic_search_service
            .expect_similar_products()
            .return_once(|_, _, _, _| Box::pin(async { Ok(Some(vec![Faker.fake()])) }));

        let lambda_event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::GET)
                .path_parameter("shopId", ShopId::new())
                .path_parameter("shopsProductId", ShopsProductId::new())
                .header("Authorization", "Bearer token")
                .build(),
            context: Default::default(),
        };

        let response = handle(
            lambda_event,
            &semantic_search_service,
            &cognito_service,
            &product_personalization_service,
        )
        .await
        .unwrap();

        assert_eq!(200, response.status_code);
        assert_eq!(
            "no-store",
            response
                .headers
                .get(CACHE_CONTROL)
                .unwrap()
                .to_str()
                .unwrap()
        );
    }

    #[tokio::test]
    async fn should_set_cache_control_to_public_when_no_user_for_get_product_similar() {
        let mut cognito_service = MockAccessTokenVerifierService::default();
        cognito_service
            .expect_verify_extract_user_id()
            .return_once(|_| Box::pin(async { Ok(None) }));
        let product_personalization_service = MockProductPersonalizationService::default();
        let mut semantic_search_service = MockSemanticSearchService::default();
        semantic_search_service
            .expect_similar_products()
            .return_once(|_, _, _, _| Box::pin(async { Ok(Some(vec![Faker.fake()])) }));

        let lambda_event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::GET)
                .path_parameter("shopId", ShopId::new())
                .path_parameter("shopsProductId", ShopsProductId::new())
                .build(),
            context: Default::default(),
        };

        let response = handle(
            lambda_event,
            &semantic_search_service,
            &cognito_service,
            &product_personalization_service,
        )
        .await
        .unwrap();

        assert_eq!(200, response.status_code);
        assert_eq!(
            "public, max-age=180, s-maxage=900",
            response
                .headers
                .get(CACHE_CONTROL)
                .unwrap()
                .to_str()
                .unwrap()
        );
    }

    #[tokio::test]
    async fn should_set_cache_control_to_public_when_similar_products_not_computed_for_get_product_similar()
     {
        let mut cognito_service = MockAccessTokenVerifierService::default();
        cognito_service
            .expect_verify_extract_user_id()
            .return_once(|_| Box::pin(async { Ok(None) }));
        let product_personalization_service = MockProductPersonalizationService::default();
        let mut semantic_search_service = MockSemanticSearchService::default();
        semantic_search_service
            .expect_similar_products()
            .return_once(|_, _, _, _| Box::pin(async { Ok(None) }));

        let lambda_event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::GET)
                .path_parameter("shopId", ShopId::new())
                .path_parameter("shopsProductId", ShopsProductId::new())
                .build(),
            context: Default::default(),
        };

        let response = handle(
            lambda_event,
            &semantic_search_service,
            &cognito_service,
            &product_personalization_service,
        )
        .await
        .unwrap();

        assert_eq!(202, response.status_code);
        assert_eq!(
            "public, max-age=300, s-maxage=900",
            response
                .headers
                .get(CACHE_CONTROL)
                .unwrap()
                .to_str()
                .unwrap()
        );
    }
}
