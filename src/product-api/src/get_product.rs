use aws_lambda_events::apigw::{ApiGatewayV2httpRequest, ApiGatewayV2httpResponse};
use cognito::access_token_verifier_service::AccessTokenVerifierService;
use common::api::api_gateway_v2_http_response_builder::ApiGatewayV2HttpResponseBuilder;
use common::api::error::ApiError;
use common::api::error_code::INTERNAL_SERVER_ERROR;
use common::currency::data::api::extract_currency_query;
use common::language::data::api::extract_language_query;
use common::language::domain::Language;
use common::personalized::api::PersonalizedData;
use common::product_id::api::extract_product_slug_id_path;
use common::shop_id::api::{extract_shop_id_path, extract_shop_slug_id_path};
use common::shops_product_id::api::extract_shops_product_id_path;
use lambda_runtime::LambdaEvent;
use product::core::product::LocalizedProductView;
use product::data::get_data::GetProductData;
use product::data::product_state_data::ProductStateData;
use product::data::user_state_data::ProductUserStateData;
use product::service::get_service::GetProductService;
use product_personalization::service::ProductPersonalizationService;

pub async fn handle(
    event: LambdaEvent<ApiGatewayV2httpRequest>,
    get_product_service: &impl GetProductService,
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
    let currency = extract_currency_query(&event.payload.query_string_parameters)?.into();

    let localized_product: LocalizedProductView = match event.payload.route_key.as_deref() {
        Some("GET /api/v1/shops/{shopId}/products/{shopsProductId}") => {
            let shop_id = extract_shop_id_path(&event.payload.path_parameters)?;
            let shops_product_id = extract_shops_product_id_path(&event.payload.path_parameters)?;
            get_product_service
                .view_product(&shop_id, &shops_product_id, languages.as_slice(), &currency)
                .await?
        }
        Some("GET /api/v1/by-slug/shops/{shopSlugId}/products/{productSlugId}") => {
            let shop_slug_id = extract_shop_slug_id_path(&event.payload.path_parameters)?;
            let product_slug_id = extract_product_slug_id_path(&event.payload.path_parameters)?;
            get_product_service
                .view_product_by_slug(
                    &shop_slug_id,
                    &product_slug_id,
                    languages.as_slice(),
                    &currency,
                )
                .await?
        }
        Some(unknown) => {
            return Err(ApiError::internal_server_error(
                INTERNAL_SERVER_ERROR,
                format!("Unknown route-key '{unknown}' in AWS-Payload").into(),
            ));
        }
        None => {
            return Err(ApiError::internal_server_error(
                INTERNAL_SERVER_ERROR,
                "Missing route-key in AWS-Payload".into(),
            ));
        }
    };

    let personalized_product_data: PersonalizedData<GetProductData, ProductUserStateData> =
        match user_id_opt {
            None => PersonalizedData {
                item: GetProductData::from(localized_product),
                user_state: None,
            },
            Some(user_id) => {
                let personalized = product_personalization_service
                    .personalize(&user_id, localized_product)
                    .await?;
                let consent = personalized
                    .user_state
                    .clone()
                    .map(|s| s.prohibited_content.consent)
                    .unwrap_or(false);
                PersonalizedData {
                    item: GetProductData::from_view(personalized.item, consent),
                    user_state: personalized.user_state.map(ProductUserStateData::from),
                }
            }
        };

    let (cache_control_directive, cache_control_max_age, cache_control_x_max_age) =
        if personalized_product_data.user_state.is_some() {
            ("no-store", None, None)
        } else if personalized_product_data.item.state == ProductStateData::Sold
            || personalized_product_data.item.state == ProductStateData::Removed
        {
            ("public", Some(180), Some(86400))
        } else {
            ("public", Some(180), Some(900))
        };

    let content_language = personalized_product_data.item.title.language;
    Ok(ApiGatewayV2HttpResponseBuilder::json(200)
        .content_language(content_language)
        .e_tag(personalized_product_data.item.event_id.to_string().as_str())
        .last_modified(personalized_product_data.item.updated)
        .cache_control(
            cache_control_directive,
            cache_control_max_age,
            cache_control_x_max_age,
        )
        .body_serde(personalized_product_data)?
        .build())
}

#[cfg(test)]
mod tests {
    use super::handle;
    use cognito::access_token_verifier_service::MockAccessTokenVerifierService;
    use common::actor::domain::Actor;
    use common::event_id::EventId;
    use common::language::data::LanguageData;
    use common::language::domain::Language;
    use common::localized::Localized;
    use common::personalized::Personalized;
    use common::product_lifecycle::domain::ProductLifecycle;
    use common::product_state::domain::ProductState;
    use common::shop_id::ShopId;
    use common::shops_product_id::ShopsProductId;
    use common::user_id::UserId;
    use fake::Fake;
    use fake::Faker;
    use http::header::{CACHE_CONTROL, CONTENT_LANGUAGE, ETAG, LAST_MODIFIED};
    use lambda_runtime::LambdaEvent;
    use product::core::product::LocalizedProductView;
    use product::core::user_state::ProductUserState;
    use product::service::get_service::{GetProductError, MockGetProductService};
    use product_personalization::service::MockProductPersonalizationService;
    use test_api::ApiGatewayV2httpRequestProxy;
    use time::OffsetDateTime;
    use time::macros::datetime;
    use url::Url;

    #[tokio::test]
    #[rstest::rstest]
    #[case(LanguageData::De, "de")]
    #[case(LanguageData::En, "en")]
    #[case(LanguageData::Es, "es")]
    #[case(LanguageData::Fr, "fr")]
    #[trace]
    async fn should_include_actual_language_as_header_content_language(
        #[case] language: LanguageData,
        #[case] expected_content_language: &str,
    ) {
        let shop_id = ShopId::new();
        let shops_product_id = ShopsProductId::new();
        let lambda_event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::GET)
                .route_key("GET /api/v1/shops/{shopId}/products/{shopsProductId}".to_owned())
                .path_parameter("shopId", shop_id)
                .path_parameter("shopsProductId", shops_product_id)
                .query_string_parameter("language", expected_content_language)
                .build(),
            context: Default::default(),
        };

        let mut cognito_service = MockAccessTokenVerifierService::default();
        cognito_service
            .expect_verify_extract_user_id()
            .return_once(|_| Box::pin(async { Ok(None) }));
        let product_personalization_service = MockProductPersonalizationService::default();
        let mut get_product_service = MockGetProductService::default();
        get_product_service.expect_view_product().return_once(
            move |shop_id, shops_product_id, _, _| {
                let product = LocalizedProductView {
                    product_id: Default::default(),
                    product_slug_id: Faker.fake(),
                    shop_slug_id: Faker.fake(),
                    seller_slug_id: Faker.fake(),
                    event_id: EventId::new(),
                    shop_id: *shop_id,
                    seller_id: Faker.fake(),
                    shops_product_id: shops_product_id.clone(),
                    shop_name: "".into(),
                    seller_name: "".into(),
                    shop_type: fake::Faker.fake(),
                    structured_address: None,
                    geo_address: None,
                    title: Localized::new(language.into(), "Native title".into()),
                    description: None,
                    price: None,
                    price_estimate_min: None,
                    price_estimate_max: None,
                    state: ProductState::Listed,
                    lifecycle: ProductLifecycle::Active,
                    url: Url::parse("https://foo.com/boop").unwrap(),
                    view_url: Url::parse(
                        "https://foo.com/boop?utm_source=aura_historia&utm_medium=referral",
                    )
                    .unwrap(),
                    images: Default::default(),
                    auction_start: None,
                    auction_end: None,
                    created_by: Actor::System,
                    updated_by: Actor::System,
                    created: OffsetDateTime::now_utc(),
                    updated: OffsetDateTime::now_utc(),
                };
                Box::pin(async move { Ok(product) })
            },
        );

        let response = handle(
            lambda_event,
            &get_product_service,
            &cognito_service,
            &product_personalization_service,
        )
        .await
        .unwrap();
        assert_eq!(200, response.status_code);
        assert_eq!(
            expected_content_language,
            response
                .headers
                .get(CONTENT_LANGUAGE)
                .unwrap()
                .to_str()
                .unwrap()
        );
    }

    #[tokio::test]
    async fn should_include_event_id_as_header_e_tag() {
        let event_id = EventId::new();
        let mut cognito_service = MockAccessTokenVerifierService::default();
        cognito_service
            .expect_verify_extract_user_id()
            .return_once(|_| Box::pin(async { Ok(None) }));
        let product_personalization_service = MockProductPersonalizationService::default();
        let mut get_product_service = MockGetProductService::default();
        get_product_service.expect_view_product().return_once(
            move |shop_id, shops_product_id, _, _| {
                let product = LocalizedProductView {
                    product_id: Default::default(),
                    product_slug_id: Faker.fake(),
                    shop_slug_id: Faker.fake(),
                    seller_slug_id: Faker.fake(),
                    event_id,
                    shop_id: *shop_id,
                    seller_id: Faker.fake(),
                    shops_product_id: shops_product_id.clone(),
                    shop_name: "".into(),
                    seller_name: "".into(),
                    structured_address: None,
                    geo_address: None,
                    title: Localized::new(Language::Es, "Native title".into()),
                    shop_type: fake::Faker.fake(),
                    description: None,
                    price: None,
                    price_estimate_min: None,
                    price_estimate_max: None,
                    state: ProductState::Listed,
                    lifecycle: ProductLifecycle::Active,
                    url: Url::parse("https://foo.com/boop").unwrap(),
                    view_url: Url::parse(
                        "https://foo.com/boop?utm_source=aura_historia&utm_medium=referral",
                    )
                    .unwrap(),
                    images: Default::default(),
                    auction_start: None,
                    auction_end: None,
                    created_by: Actor::System,
                    updated_by: Actor::System,
                    created: OffsetDateTime::now_utc(),
                    updated: OffsetDateTime::now_utc(),
                };
                Box::pin(async move { Ok(product) })
            },
        );
        let shop_id = ShopId::new();
        let shops_product_id = ShopsProductId::new();
        let lambda_event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::GET)
                .route_key("GET /api/v1/shops/{shopId}/products/{shopsProductId}".to_owned())
                .path_parameter("shopId", shop_id)
                .path_parameter("shopsProductId", shops_product_id)
                .build(),
            context: Default::default(),
        };
        let response = handle(
            lambda_event,
            &get_product_service,
            &cognito_service,
            &product_personalization_service,
        )
        .await
        .unwrap();
        assert_eq!(200, response.status_code);
        assert_eq!(
            event_id.to_string().as_str(),
            response.headers.get(ETAG).unwrap()
        );
    }

    #[tokio::test]
    async fn should_include_updated_timestamp_as_header_last_modified() {
        let timestamp = datetime!(2020-01-01 0:00 UTC);
        let event_id = EventId::new();
        let mut cognito_service = MockAccessTokenVerifierService::default();
        cognito_service
            .expect_verify_extract_user_id()
            .return_once(|_| Box::pin(async { Ok(None) }));
        let product_personalization_service = MockProductPersonalizationService::default();
        let mut get_product_service = MockGetProductService::default();
        get_product_service.expect_view_product().return_once(
            move |shop_id, shops_product_id, _, _| {
                let product = LocalizedProductView {
                    product_id: Default::default(),
                    product_slug_id: Faker.fake(),
                    shop_slug_id: Faker.fake(),
                    seller_slug_id: Faker.fake(),
                    event_id,
                    shop_id: *shop_id,
                    seller_id: Faker.fake(),
                    shops_product_id: shops_product_id.clone(),
                    shop_name: "".into(),
                    seller_name: "".into(),
                    structured_address: None,
                    geo_address: None,
                    title: Localized::new(Language::Es, "Native title".into()),
                    description: None,
                    shop_type: fake::Faker.fake(),
                    price: None,
                    price_estimate_min: None,
                    price_estimate_max: None,
                    state: ProductState::Listed,
                    lifecycle: ProductLifecycle::Active,
                    url: Url::parse("https://foo.com/boop").unwrap(),
                    view_url: Url::parse(
                        "https://foo.com/boop?utm_source=aura_historia&utm_medium=referral",
                    )
                    .unwrap(),
                    images: Default::default(),
                    auction_start: None,
                    auction_end: None,
                    created_by: Actor::System,
                    updated_by: Actor::System,
                    created: timestamp,
                    updated: timestamp,
                };
                Box::pin(async move { Ok(product) })
            },
        );
        let shop_id = ShopId::new();
        let shops_product_id = ShopsProductId::new();
        let lambda_event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::GET)
                .route_key("GET /api/v1/shops/{shopId}/products/{shopsProductId}".to_owned())
                .path_parameter("shopId", shop_id)
                .path_parameter("shopsProductId", shops_product_id)
                .build(),
            context: Default::default(),
        };
        let response = handle(
            lambda_event,
            &get_product_service,
            &cognito_service,
            &product_personalization_service,
        )
        .await
        .unwrap();
        assert_eq!(200, response.status_code);
        assert_eq!(
            "Wed, 01 Jan 2020 00:00:00 GMT",
            response.headers.get(LAST_MODIFIED).unwrap()
        );
    }

    #[tokio::test]
    async fn should_400_when_path_param_shop_id_is_missing() {
        let mut cognito_service = MockAccessTokenVerifierService::default();
        cognito_service
            .expect_verify_extract_user_id()
            .return_once(|_| Box::pin(async { Ok(None) }));
        let product_personalization_service = MockProductPersonalizationService::default();
        let mut get_product_service = MockGetProductService::default();
        get_product_service.expect_view_product().never();
        let lambda_event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::GET)
                .route_key("GET /api/v1/shops/{shopId}/products/{shopsProductId}".to_owned())
                .path_parameter("shopsProductId", ShopsProductId::new())
                .build(),
            context: Default::default(),
        };

        let actual = handle(
            lambda_event,
            &get_product_service,
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
        let mut get_product_service = MockGetProductService::default();
        get_product_service.expect_view_product().never();
        let lambda_event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::GET)
                .route_key("GET /api/v1/shops/{shopId}/products/{shopsProductId}".to_owned())
                .path_parameter("shopId", ShopId::new())
                .build(),
            context: Default::default(),
        };

        let actual = handle(
            lambda_event,
            &get_product_service,
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
                .route_key("GET /api/v1/shops/{shopId}/products/{shopsProductId}".to_owned())
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
        let mut get_product_service = MockGetProductService::default();
        get_product_service.expect_view_product().return_once(
            move |shop_id, shops_product_id, _, _| {
                let shop_id = *shop_id;
                let shops_product_id = shops_product_id.clone();
                Box::pin(
                    async move { Err(GetProductError::ProductNotFound(shop_id, shops_product_id)) },
                )
            },
        );

        let actual = handle(
            lambda_event,
            &get_product_service,
            &cognito_service,
            &product_personalization_service,
        )
        .await
        .unwrap_err();
        assert_eq!(404, actual.status);
    }

    #[tokio::test]
    async fn should_set_cache_control_to_no_store_when_user_state_present_for_get_product() {
        let mut cognito_service = MockAccessTokenVerifierService::default();
        cognito_service
            .expect_verify_extract_user_id()
            .return_once(|_| Box::pin(async { Ok(Some(UserId::new())) }));
        let mut product_personalization_service = MockProductPersonalizationService::default();
        product_personalization_service
            .expect_personalize()
            .return_once(|_, product| {
                use product::core::user_state::WatchlistUserState;
                let personalized = Personalized {
                    item: product,
                    user_state: Some(ProductUserState {
                        watchlist: WatchlistUserState {
                            watching: true,
                            notifications: false,
                        },
                        ..Default::default()
                    }),
                };
                Box::pin(async move { Ok(personalized) })
            });
        let mut get_product_service = MockGetProductService::default();
        get_product_service.expect_view_product().return_once(
            move |shop_id, shops_product_id, _, _| {
                let product = LocalizedProductView {
                    product_id: Default::default(),
                    product_slug_id: Faker.fake(),
                    shop_slug_id: Faker.fake(),
                    seller_slug_id: Faker.fake(),
                    event_id: EventId::new(),
                    shop_id: *shop_id,
                    seller_id: Faker.fake(),
                    shops_product_id: shops_product_id.clone(),
                    shop_name: "".into(),
                    seller_name: "".into(),
                    shop_type: fake::Faker.fake(),
                    structured_address: None,
                    geo_address: None,
                    title: Localized::new(Language::En, "Native title".into()),
                    description: None,
                    price: None,
                    price_estimate_min: None,
                    price_estimate_max: None,
                    state: ProductState::Listed,
                    lifecycle: ProductLifecycle::Active,
                    url: Url::parse("https://foo.com/boop").unwrap(),
                    view_url: Url::parse(
                        "https://foo.com/boop?utm_source=aura_historia&utm_medium=referral",
                    )
                    .unwrap(),
                    images: Default::default(),
                    auction_start: None,
                    auction_end: None,
                    created_by: Actor::System,
                    updated_by: Actor::System,
                    created: OffsetDateTime::now_utc(),
                    updated: OffsetDateTime::now_utc(),
                };
                Box::pin(async move { Ok(product) })
            },
        );
        let shop_id = ShopId::new();
        let shops_product_id = ShopsProductId::new();
        let lambda_event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::GET)
                .route_key("GET /api/v1/shops/{shopId}/products/{shopsProductId}".to_owned())
                .path_parameter("shopId", shop_id)
                .path_parameter("shopsProductId", shops_product_id)
                .header("Authorization", "Bearer token")
                .build(),
            context: Default::default(),
        };

        let response = handle(
            lambda_event,
            &get_product_service,
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
    async fn should_set_cache_control_with_long_s_maxage_when_product_is_sold_for_get_product() {
        let mut cognito_service = MockAccessTokenVerifierService::default();
        cognito_service
            .expect_verify_extract_user_id()
            .return_once(|_| Box::pin(async { Ok(None) }));
        let product_personalization_service = MockProductPersonalizationService::default();
        let mut get_product_service = MockGetProductService::default();
        get_product_service.expect_view_product().return_once(
            move |shop_id, shops_product_id, _, _| {
                let product = LocalizedProductView {
                    product_id: Default::default(),
                    product_slug_id: Faker.fake(),
                    shop_slug_id: Faker.fake(),
                    seller_slug_id: Faker.fake(),
                    event_id: EventId::new(),
                    shop_id: *shop_id,
                    seller_id: Faker.fake(),
                    shops_product_id: shops_product_id.clone(),
                    shop_name: "".into(),
                    seller_name: "".into(),
                    shop_type: fake::Faker.fake(),
                    structured_address: None,
                    geo_address: None,
                    title: Localized::new(Language::En, "Native title".into()),
                    description: None,
                    price: None,
                    price_estimate_min: None,
                    price_estimate_max: None,
                    state: ProductState::Sold,
                    lifecycle: ProductLifecycle::Active,
                    url: Url::parse("https://foo.com/boop").unwrap(),
                    view_url: Url::parse(
                        "https://foo.com/boop?utm_source=aura_historia&utm_medium=referral",
                    )
                    .unwrap(),
                    images: Default::default(),
                    auction_start: None,
                    auction_end: None,
                    created_by: Actor::System,
                    updated_by: Actor::System,
                    created: OffsetDateTime::now_utc(),
                    updated: OffsetDateTime::now_utc(),
                };
                Box::pin(async move { Ok(product) })
            },
        );
        let shop_id = ShopId::new();
        let shops_product_id = ShopsProductId::new();
        let lambda_event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::GET)
                .route_key("GET /api/v1/shops/{shopId}/products/{shopsProductId}".to_owned())
                .path_parameter("shopId", shop_id)
                .path_parameter("shopsProductId", shops_product_id)
                .build(),
            context: Default::default(),
        };

        let response = handle(
            lambda_event,
            &get_product_service,
            &cognito_service,
            &product_personalization_service,
        )
        .await
        .unwrap();

        assert_eq!(200, response.status_code);
        assert_eq!(
            "public, max-age=180, s-maxage=86400",
            response
                .headers
                .get(CACHE_CONTROL)
                .unwrap()
                .to_str()
                .unwrap()
        );
    }

    #[tokio::test]
    async fn should_set_cache_control_with_long_s_maxage_when_product_is_removed_for_get_product() {
        let mut cognito_service = MockAccessTokenVerifierService::default();
        cognito_service
            .expect_verify_extract_user_id()
            .return_once(|_| Box::pin(async { Ok(None) }));
        let product_personalization_service = MockProductPersonalizationService::default();
        let mut get_product_service = MockGetProductService::default();
        get_product_service.expect_view_product().return_once(
            move |shop_id, shops_product_id, _, _| {
                let product = LocalizedProductView {
                    product_id: Default::default(),
                    product_slug_id: Faker.fake(),
                    shop_slug_id: Faker.fake(),
                    seller_slug_id: Faker.fake(),
                    event_id: EventId::new(),
                    shop_id: *shop_id,
                    seller_id: Faker.fake(),
                    shops_product_id: shops_product_id.clone(),
                    shop_name: "".into(),
                    seller_name: "".into(),
                    shop_type: fake::Faker.fake(),
                    structured_address: None,
                    geo_address: None,
                    title: Localized::new(Language::En, "Native title".into()),
                    description: None,
                    price: None,
                    price_estimate_min: None,
                    price_estimate_max: None,
                    state: ProductState::Removed,
                    lifecycle: ProductLifecycle::Active,
                    url: Url::parse("https://foo.com/boop").unwrap(),
                    view_url: Url::parse(
                        "https://foo.com/boop?utm_source=aura_historia&utm_medium=referral",
                    )
                    .unwrap(),
                    images: Default::default(),
                    auction_start: None,
                    auction_end: None,
                    created_by: Actor::System,
                    updated_by: Actor::System,
                    created: OffsetDateTime::now_utc(),
                    updated: OffsetDateTime::now_utc(),
                };
                Box::pin(async move { Ok(product) })
            },
        );
        let shop_id = ShopId::new();
        let shops_product_id = ShopsProductId::new();
        let lambda_event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::GET)
                .route_key("GET /api/v1/shops/{shopId}/products/{shopsProductId}".to_owned())
                .path_parameter("shopId", shop_id)
                .path_parameter("shopsProductId", shops_product_id)
                .build(),
            context: Default::default(),
        };

        let response = handle(
            lambda_event,
            &get_product_service,
            &cognito_service,
            &product_personalization_service,
        )
        .await
        .unwrap();

        assert_eq!(200, response.status_code);
        assert_eq!(
            "public, max-age=180, s-maxage=86400",
            response
                .headers
                .get(CACHE_CONTROL)
                .unwrap()
                .to_str()
                .unwrap()
        );
    }

    #[tokio::test]
    async fn should_set_cache_control_with_standard_s_maxage_when_product_is_not_sold_for_get_product()
     {
        let mut cognito_service = MockAccessTokenVerifierService::default();
        cognito_service
            .expect_verify_extract_user_id()
            .return_once(|_| Box::pin(async { Ok(None) }));
        let product_personalization_service = MockProductPersonalizationService::default();
        let mut get_product_service = MockGetProductService::default();
        get_product_service.expect_view_product().return_once(
            move |shop_id, shops_product_id, _, _| {
                let product = LocalizedProductView {
                    product_id: Default::default(),
                    product_slug_id: Faker.fake(),
                    shop_slug_id: Faker.fake(),
                    seller_slug_id: Faker.fake(),
                    event_id: EventId::new(),
                    shop_id: *shop_id,
                    seller_id: Faker.fake(),
                    shops_product_id: shops_product_id.clone(),
                    shop_name: "".into(),
                    seller_name: "".into(),
                    shop_type: fake::Faker.fake(),
                    structured_address: None,
                    geo_address: None,
                    title: Localized::new(Language::En, "Native title".into()),
                    description: None,
                    price: None,
                    price_estimate_min: None,
                    price_estimate_max: None,
                    state: ProductState::Listed,
                    lifecycle: ProductLifecycle::Active,
                    url: Url::parse("https://foo.com/boop").unwrap(),
                    view_url: Url::parse(
                        "https://foo.com/boop?utm_source=aura_historia&utm_medium=referral",
                    )
                    .unwrap(),
                    images: Default::default(),
                    auction_start: None,
                    auction_end: None,
                    created_by: Actor::System,
                    updated_by: Actor::System,
                    created: OffsetDateTime::now_utc(),
                    updated: OffsetDateTime::now_utc(),
                };
                Box::pin(async move { Ok(product) })
            },
        );
        let shop_id = ShopId::new();
        let shops_product_id = ShopsProductId::new();
        let lambda_event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::GET)
                .route_key("GET /api/v1/shops/{shopId}/products/{shopsProductId}".to_owned())
                .path_parameter("shopId", shop_id)
                .path_parameter("shopsProductId", shops_product_id)
                .build(),
            context: Default::default(),
        };

        let response = handle(
            lambda_event,
            &get_product_service,
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
}
