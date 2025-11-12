use aws_lambda_events::apigw::{ApiGatewayV2httpRequest, ApiGatewayV2httpResponse};
use aws_lambda_events::query_map::QueryMap;
use cognito::access_token_verifier_service::AccessTokenVerifierService;
use common::api::api_gateway_v2_http_response_builder::ApiGatewayV2HttpResponseBuilder;
use common::api::error::{ApiError, log_api_error};
use common::api::error_code::BAD_QUERY_PARAMETER_VALUE;
use common::currency::data::api::extract_currency_query;
use common::language::data::api::extract_languages_header;
use common::language::domain::Language;
use common::personalized::Personalized;
use common::personalized::api::PersonalizedData;
use common::shop_id::api::extract_shop_id_path;
use common::shops_product_id::api::extract_shops_item_id_path;
use lambda_runtime::LambdaEvent;
use product::core::product::LocalizedProductView;
use product::core::user_state::ItemUserState;
use product::data::get_data::GetItemData;
use product::data::user_state_data::ItemUserStateData;
use product::service::get_service::GetProductService;
use product::service::personalization_service::ItemPersonalizationService;

#[tracing::instrument(
    skip(event, get_item_service, access_token_verifier_service, item_personalization_service),
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
    get_item_service: &impl GetProductService,
    access_token_verifier_service: &(impl AccessTokenVerifierService + Sync),
    item_personalization_service: &impl ItemPersonalizationService,
) -> Result<ApiGatewayV2httpResponse, lambda_runtime::Error> {
    match handle(
        event,
        get_item_service,
        access_token_verifier_service,
        item_personalization_service,
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

// GET /api/v1/items/{shopId}/{shopsItemId}
pub async fn handle(
    event: LambdaEvent<ApiGatewayV2httpRequest>,
    get_item_service: &impl GetProductService,
    access_token_verifier_service: &(impl AccessTokenVerifierService + Sync),
    item_personalization_service: &impl ItemPersonalizationService,
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
    let currency = extract_currency_query(&event.payload.query_string_parameters)?.into();
    let shop_id = extract_shop_id_path(&event.payload.path_parameters)?;
    let shops_product_id = extract_shops_item_id_path(&event.payload.path_parameters)?;

    let localized_item: LocalizedProductView = get_item_service
        .view_item(
            &shop_id,
            &shops_product_id,
            languages.as_slice(),
            &currency,
            extract_history_query(&event.payload.query_string_parameters)?,
        )
        .await?;
    let personalized_item_data: PersonalizedData<GetItemData, ItemUserStateData> = match user_id_opt
    {
        None => PersonalizedData {
            item: GetItemData::from(localized_item),
            user_state: None,
        },
        Some(user_id) => item_personalization_service
            .personalize_watchlist(&user_id, localized_item)
            .await
            .map(|personalized_watchlist| Personalized {
                item: personalized_watchlist.item,
                user_state: personalized_watchlist
                    .user_state
                    .map(|watchlist| ItemUserState { watchlist }),
            })?
            .into(),
    };

    let content_language = personalized_item_data.item.title.language;
    Ok(ApiGatewayV2HttpResponseBuilder::json(200)
        .content_language(content_language)
        .e_tag(personalized_item_data.item.event_id.to_string().as_str())
        .last_modified(personalized_item_data.item.updated)
        .body_serde(personalized_item_data)?
        .build())
}

fn extract_history_query(query: &QueryMap) -> Result<bool, ApiError> {
    query
        .first("history")
        .map(|val| match val {
            "true" => Ok(true),
            "false" => Ok(false),
            other => {
                let err_msg = format!("Expected any of: 'true' or 'false'. Got: '{other}'");
                Err(
                    ApiError::bad_request(BAD_QUERY_PARAMETER_VALUE, err_msg.as_str().into())
                        .with_query_field("history")
                        .with_message(err_msg),
                )
            }
        })
        .unwrap_or(Ok(false))
}

#[cfg(test)]
mod tests {
    use crate::handler;
    use cognito::access_token_verifier_service::MockAccessTokenVerifierService;
    use common::event_id::EventId;
    use common::language::data::LanguageData;
    use common::language::domain::Language;
    use common::localized::Localized;
    use common::product_state::domain::ProductState;
    use common::shop_id::ShopId;
    use common::shops_product_id::ShopsProductId;
    use http::header::{ACCEPT_LANGUAGE, CONTENT_LANGUAGE, ETAG, LAST_MODIFIED};
    use lambda_runtime::LambdaEvent;
    use product::core::product::LocalizedProductView;
    use product::service::get_service::{GetProductError, MockGetItemService};
    use product::service::personalization_service::MockItemPersonalizationService;
    use test_api::{ApiGatewayV2httpRequestProxy, extract_apigw_response_json_body};
    use time::OffsetDateTime;
    use time::macros::datetime;
    use url::Url;

    #[tokio::test]
    #[rstest::rstest]
    #[case(LanguageData::De, "de")]
    #[case(LanguageData::En, "en")]
    #[case(LanguageData::Es, "es")]
    #[case(LanguageData::Fr, "fr")]
    async fn should_include_actual_language_as_header_content_language(
        #[case] language: LanguageData,
        #[case] expected_content_language: &str,
    ) {
        let shop_id = ShopId::new();
        let shops_product_id = ShopsProductId::new();
        let lambda_event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::GET)
                .path_parameter("shopId", shop_id)
                .path_parameter("shopsProductId", shops_product_id)
                .header(ACCEPT_LANGUAGE.as_str(), expected_content_language)
                .build(),
            context: Default::default(),
        };

        let mut cognito_service = MockAccessTokenVerifierService::default();
        cognito_service
            .expect_verify_extract_user_id()
            .return_once(|_| Box::pin(async { Ok(None) }));
        let item_personalization_service = MockItemPersonalizationService::default();
        let mut get_item_service = MockGetItemService::default();
        get_item_service.expect_view_item().return_once(
            move |shop_id, shops_product_id, _, _, _| {
                let item = LocalizedProductView {
                    product_id: Default::default(),
                    event_id: EventId::new(),
                    shop_id: *shop_id,
                    shops_product_id: shops_product_id.clone(),
                    shop_name: "".into(),
                    title: Localized::new(language.into(), "Native title".into()),
                    description: None,
                    price: None,
                    state: ProductState::Listed,
                    url: Url::parse("https://foo.com/boop").unwrap(),
                    images: vec![],
                    created: OffsetDateTime::now_utc(),
                    updated: OffsetDateTime::now_utc(),
                    history: None,
                };
                Box::pin(async move { Ok(item) })
            },
        );

        let response = handler(
            lambda_event,
            &get_item_service,
            &cognito_service,
            &item_personalization_service,
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
        let item_personalization_service = MockItemPersonalizationService::default();
        let mut get_item_service = MockGetItemService::default();
        get_item_service.expect_view_item().return_once(
            move |shop_id, shops_product_id, _, _, _| {
                let item = LocalizedProductView {
                    product_id: Default::default(),
                    event_id,
                    shop_id: *shop_id,
                    shops_product_id: shops_product_id.clone(),
                    shop_name: "".into(),
                    title: Localized::new(Language::Es, "Native title".into()),
                    description: None,
                    price: None,
                    state: ProductState::Listed,
                    url: Url::parse("https://foo.com/boop").unwrap(),
                    images: vec![],
                    created: OffsetDateTime::now_utc(),
                    updated: OffsetDateTime::now_utc(),
                    history: None,
                };
                Box::pin(async move { Ok(item) })
            },
        );
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
        let response = handler(
            lambda_event,
            &get_item_service,
            &cognito_service,
            &item_personalization_service,
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
        let item_personalization_service = MockItemPersonalizationService::default();
        let mut get_item_service = MockGetItemService::default();
        get_item_service.expect_view_item().return_once(
            move |shop_id, shops_product_id, _, _, _| {
                let item = LocalizedProductView {
                    product_id: Default::default(),
                    event_id,
                    shop_id: *shop_id,
                    shops_product_id: shops_product_id.clone(),
                    shop_name: "".into(),
                    title: Localized::new(Language::Es, "Native title".into()),
                    description: None,
                    price: None,
                    state: ProductState::Listed,
                    url: Url::parse("https://foo.com/boop").unwrap(),
                    images: vec![],
                    created: timestamp,
                    updated: timestamp,
                    history: None,
                };
                Box::pin(async move { Ok(item) })
            },
        );
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
        let response = handler(
            lambda_event,
            &get_item_service,
            &cognito_service,
            &item_personalization_service,
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
    #[rstest::rstest]
    #[case::default_false(None, false)]
    #[case::accept_true(Some("true"), true)]
    #[case::accept_false(Some("false"), false)]
    async fn should_respect_history_query_param(
        #[case] history_query_value: Option<&'static str>,
        #[case] expected_history: bool,
    ) {
        let timestamp = datetime!(2020-01-01 0:00 UTC);
        let event_id = EventId::new();
        let mut cognito_service = MockAccessTokenVerifierService::default();
        cognito_service
            .expect_verify_extract_user_id()
            .return_once(|_| Box::pin(async { Ok(None) }));
        let item_personalization_service = MockItemPersonalizationService::default();
        let mut get_item_service = MockGetItemService::default();
        get_item_service.expect_view_item().return_once(
            move |shop_id, shops_product_id, _, _, history| {
                assert_eq!(expected_history, history);
                let item = LocalizedProductView {
                    product_id: Default::default(),
                    event_id,
                    shop_id: *shop_id,
                    shops_product_id: shops_product_id.clone(),
                    shop_name: "".into(),
                    title: Localized::new(Language::Es, "Native title".into()),
                    description: None,
                    price: None,
                    state: ProductState::Listed,
                    url: Url::parse("https://foo.com/boop").unwrap(),
                    images: vec![],
                    created: timestamp,
                    updated: timestamp,
                    history: None,
                };
                Box::pin(async move { Ok(item) })
            },
        );
        let shop_id = ShopId::new();
        let shops_product_id = ShopsProductId::new();
        let lambda_event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::GET)
                .path_parameter("shopId", shop_id)
                .path_parameter("shopsProductId", shops_product_id)
                .try_query_string_parameter("history", history_query_value)
                .build(),
            context: Default::default(),
        };
        let response = handler(
            lambda_event,
            &get_item_service,
            &cognito_service,
            &item_personalization_service,
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
        let item_personalization_service = MockItemPersonalizationService::default();
        let mut get_item_service = MockGetItemService::default();
        get_item_service.expect_view_item().never();
        let lambda_event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::GET)
                .path_parameter("shopsProductId", ShopsProductId::new())
                .build(),
            context: Default::default(),
        };

        let response = handler(
            lambda_event,
            &get_item_service,
            &cognito_service,
            &item_personalization_service,
        )
        .await
        .unwrap();
        assert_eq!(400, response.status_code);
        let json = extract_apigw_response_json_body!(response);
        assert_eq!(400, json["status"]);
        assert_eq!("shopId", json["source"]["field"]);
    }

    #[tokio::test]
    async fn should_400_when_path_param_shops_item_id_is_missing() {
        let mut cognito_service = MockAccessTokenVerifierService::default();
        cognito_service
            .expect_verify_extract_user_id()
            .return_once(|_| Box::pin(async { Ok(None) }));
        let item_personalization_service = MockItemPersonalizationService::default();
        let mut get_item_service = MockGetItemService::default();
        get_item_service.expect_view_item().never();
        let lambda_event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::GET)
                .path_parameter("shopId", ShopId::new())
                .build(),
            context: Default::default(),
        };

        let response = handler(
            lambda_event,
            &get_item_service,
            &cognito_service,
            &item_personalization_service,
        )
        .await
        .unwrap();
        assert_eq!(400, response.status_code);
        let json = extract_apigw_response_json_body!(response);
        assert_eq!(400, json["status"]);
        assert_eq!("shopsProductId", json["source"]["field"]);
    }

    #[tokio::test]
    async fn should_400_when_history_query_param_value_invalid() {
        let mut cognito_service = MockAccessTokenVerifierService::default();
        cognito_service
            .expect_verify_extract_user_id()
            .return_once(|_| Box::pin(async { Ok(None) }));
        let item_personalization_service = MockItemPersonalizationService::default();
        let mut get_item_service = MockGetItemService::default();
        get_item_service.expect_view_item().never();
        let lambda_event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::GET)
                .path_parameter("shopId", ShopId::new())
                .path_parameter("shopsProductId", ShopsProductId::new())
                .query_string_parameter("history", "boop")
                .build(),
            context: Default::default(),
        };

        let response = handler(
            lambda_event,
            &get_item_service,
            &cognito_service,
            &item_personalization_service,
        )
        .await
        .unwrap();
        assert_eq!(400, response.status_code);
        let json = extract_apigw_response_json_body!(response);
        assert_eq!(400, json["status"]);
        assert_eq!("history", json["source"]["field"]);
    }

    #[tokio::test]
    async fn should_404_when_item_does_not_exist() {
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
        let item_personalization_service = MockItemPersonalizationService::default();
        let mut get_item_service = MockGetItemService::default();
        get_item_service.expect_view_item().return_once(
            move |shop_id, shops_product_id, _, _, _| {
                let shop_id = *shop_id;
                let shops_product_id = shops_product_id.clone();
                Box::pin(
                    async move { Err(GetProductError::ItemNotFound(shop_id, shops_product_id)) },
                )
            },
        );

        let response = handler(
            lambda_event,
            &get_item_service,
            &cognito_service,
            &item_personalization_service,
        )
        .await
        .unwrap();
        assert_eq!(404, response.status_code);
        let json = extract_apigw_response_json_body!(response);
        assert_eq!(404, json["status"]);
    }
}
