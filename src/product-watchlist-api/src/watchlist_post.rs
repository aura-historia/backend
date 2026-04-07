use aws_lambda_events::apigw::{ApiGatewayV2httpRequest, ApiGatewayV2httpResponse};
use common::api::api_gateway_v2_http_response_builder::ApiGatewayV2HttpResponseBuilder;
use common::api::error::ApiError;
use common::api::error_code::BAD_BODY_VALUE;
use common::currency::data::api::extract_currency_query;
use common::language::data::api::extract_language_query;
use common::personalized::api::PersonalizedData;
use common::product_id::api::ProductKeyData;
use common::user_id::api::extract_user_id_request_context;
use lambda_runtime::LambdaEvent;
use product::data::get_data::GetProductData;
use product::data::user_state_data::ProductUserStateData;
use product::service::get_service::GetProductService;
use product_personalization::service::ProductPersonalizationService;
use product_watchlist::service::product_watchlist_service::ProductWatchListService;

pub async fn handle(
    event: LambdaEvent<ApiGatewayV2httpRequest>,
    watchlist_service: &impl ProductWatchListService,
    get_product_service: &impl GetProductService,
    personalization_service: &impl ProductPersonalizationService,
) -> Result<ApiGatewayV2httpResponse, ApiError> {
    let user_id = extract_user_id_request_context(&event.payload.request_context)?;
    tracing::Span::current().record("userId", user_id.to_string());
    let language = extract_language_query(&event.payload.query_string_parameters)?;
    let currency = extract_currency_query(&event.payload.query_string_parameters)?;
    let body = event
        .payload
        .body
        .filter(|str| !str.is_empty())
        .ok_or_else(|| {
            let err_msg = "Body cannot be empty";
            ApiError::bad_request(BAD_BODY_VALUE, err_msg.into()).with_detail(err_msg)
        })?;
    let product_key_data: ProductKeyData = serde_json::from_str(&body).map_err(|err| {
        let err_msg = err.to_string();
        ApiError::bad_request(BAD_BODY_VALUE, Box::new(err)).with_detail(err_msg)
    })?;

    // 1. Create watchlist entry
    watchlist_service
        .create_watchlist_product(
            &user_id,
            &product_key_data.shop_id,
            &product_key_data.shops_product_id,
        )
        .await?;

    // 2. Get localized product view
    let localized_product = get_product_service
        .view_product(
            &product_key_data.shop_id,
            &product_key_data.shops_product_id,
            &[language.into()],
            &currency.into(),
        )
        .await?;

    // 3. Personalize for the authenticated user
    let personalized = personalization_service
        .personalize(&user_id, localized_product)
        .await?;

    let consent = personalized
        .user_state
        .clone()
        .map(|s| s.prohibited_content.consent)
        .unwrap_or(false);
    let response_data = PersonalizedData {
        item: GetProductData::from_view(personalized.item, consent),
        user_state: personalized.user_state.map(ProductUserStateData::from),
    };

    Ok(ApiGatewayV2HttpResponseBuilder::json(201)
        .location(
            &format!(
                "me/watchlist/{}/{}",
                product_key_data.shop_id, product_key_data.shops_product_id
            ),
            &event.payload.request_context,
        )
        .body_serde(response_data)?
        .build())
}

#[cfg(test)]
mod tests {
    use super::handle;
    use common::{personalized::Personalized, product_id::api::ProductKeyData, user_id::UserId};
    use fake::{Fake, Faker};
    use lambda_runtime::LambdaEvent;
    use product::{
        core::product::LocalizedProductView, service::get_service::MockGetProductService,
    };
    use product_personalization::service::MockProductPersonalizationService;
    use product_watchlist::service::product_watchlist_service::MockProductWatchListService;
    use test_api::ApiGatewayV2httpRequestProxy;

    #[tokio::test]
    async fn should_201_when_success() {
        let mut watchlist_service = MockProductWatchListService::default();
        watchlist_service
            .expect_create_watchlist_product()
            .return_once(|_, _, _| Box::pin(async { Ok(Faker.fake()) }));
        let mut get_product_service = MockGetProductService::default();
        get_product_service
            .expect_view_product()
            .return_once(|_, _, _, _| Box::pin(async { Ok(Faker.fake()) }));
        let mut personalization_service = MockProductPersonalizationService::default();
        personalization_service
            .expect_personalize()
            .return_once(|_, _| {
                Box::pin(async {
                    Ok(Personalized {
                        item: Faker.fake::<LocalizedProductView>(),
                        user_state: None,
                    })
                })
            });

        let product_key_data = Faker.fake::<ProductKeyData>();
        let lambda_event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::POST)
                .body_serde(&product_key_data)
                .jwt_claim("sub", UserId::new())
                .query_string_parameter("language", "de")
                .query_string_parameter("currency", "EUR")
                .stage("prod")
                .build(),
            context: Default::default(),
        };

        let response = handle(
            lambda_event,
            &watchlist_service,
            &get_product_service,
            &personalization_service,
        )
        .await
        .unwrap();

        assert_eq!(201, response.status_code);
    }

    #[tokio::test]
    async fn should_401_when_sub_missing() {
        let mut watchlist_service = MockProductWatchListService::default();
        watchlist_service.expect_create_watchlist_product().never();
        let get_product_service = MockGetProductService::default();
        let personalization_service = MockProductPersonalizationService::default();

        let lambda_event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::POST)
                .body_serde(&Faker.fake::<ProductKeyData>())
                .build(),
            context: Default::default(),
        };

        let actual = handle(
            lambda_event,
            &watchlist_service,
            &get_product_service,
            &personalization_service,
        )
        .await
        .unwrap_err();
        assert_eq!(401, actual.status);
    }
}
