use aws_lambda_events::apigw::{ApiGatewayV2httpRequest, ApiGatewayV2httpResponse};
use cognito::access_token_verifier_service::AccessTokenVerifierService;
use common::api::api_gateway_v2_http_response_builder::ApiGatewayV2HttpResponseBuilder;
use common::api::error::ApiError;
use common::api::error_code::BAD_BODY_VALUE;
use common::currency::domain::Currency;
use common::language::domain::Language;
use lambda_runtime::LambdaEvent;
use user::service::user_service::UserService;

use crate::data::PutNewsletterSubscriptionData;
use crate::domain::UpsertNewsletterSubscription;
use crate::service::ZohoCampaignsService;

pub async fn handle(
    event: LambdaEvent<ApiGatewayV2httpRequest>,
    zoho_campaigns_service: &(impl ZohoCampaignsService + Sync),
    access_token_verifier_service: &(impl AccessTokenVerifierService + Sync),
    user_service: &(impl UserService + Sync),
) -> Result<ApiGatewayV2httpResponse, ApiError> {
    let user_id_opt = access_token_verifier_service
        .verify_extract_user_id(&event.payload.headers)
        .await
        .map_err(crate::map_access_token_error)?;
    if let Some(user_id) = user_id_opt {
        tracing::Span::current().record("userId", user_id.to_string());
    }

    let body = event
        .payload
        .body
        .filter(|str| !str.is_empty())
        .ok_or_else(|| {
            let err_msg = "Body cannot be empty";
            ApiError::bad_request(BAD_BODY_VALUE, err_msg.into()).with_detail(err_msg)
        })?;

    let put_data: PutNewsletterSubscriptionData = serde_json::from_str(&body).map_err(|err| {
        let err_msg = err.to_string();
        ApiError::bad_request(BAD_BODY_VALUE, Box::new(err)).with_detail(err_msg)
    })?;

    let (fallback_first_name, fallback_last_name, fallback_language, fallback_currency) =
        if let Some(user_id) = user_id_opt {
            match user_service.find_user(&user_id).await {
                Ok(user) => (
                    user.first_name,
                    user.last_name,
                    user.language,
                    user.currency,
                ),
                Err(err) => {
                    tracing::debug!(userId = %user_id, error = ?err, "Failed to find user for newsletter fallback values.");
                    (None, None, None, None)
                }
            }
        } else {
            (None, None, None, None)
        };

    let subscription = UpsertNewsletterSubscription {
        email: put_data.email,
        first_name: put_data.first_name.or(fallback_first_name),
        last_name: put_data.last_name.or(fallback_last_name),
        language: put_data.language.map(Language::from).or(fallback_language),
        currency: put_data.currency.map(Currency::from).or(fallback_currency),
        user_id: user_id_opt,
    };

    zoho_campaigns_service.subscribe(&subscription).await?;

    Ok(ApiGatewayV2HttpResponseBuilder::new(204).build())
}

#[cfg(test)]
mod tests {
    use crate::handle;
    use crate::service::MockZohoCampaignsService;
    use cognito::access_token_verifier_service::MockAccessTokenVerifierService;
    use common::user_id::UserId;
    use lambda_runtime::LambdaEvent;
    use test_api::ApiGatewayV2httpRequestProxy;
    use user::service::user_service::MockUserService;

    use crate::data::PutNewsletterSubscriptionData;

    #[tokio::test]
    async fn should_return_204_when_subscription_succeeds() {
        let mut zoho_service = MockZohoCampaignsService::default();
        zoho_service
            .expect_subscribe()
            .return_once(|_| Box::pin(async { Ok(()) }));

        let mut access_token_service = MockAccessTokenVerifierService::default();
        access_token_service
            .expect_verify_extract_user_id()
            .return_once(|_| Box::pin(async { Ok(None) }));

        let user_service = MockUserService::default();

        let data = PutNewsletterSubscriptionData {
            email: "test@example.com".try_into().unwrap(),
            first_name: Some("Test".into()),
            last_name: Some("User".into()),
            language: None,
            currency: None,
        };

        let lambda_event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::PUT)
                .route_key("PUT /api/v1/newsletter-subscriptions")
                .body_serde(&data)
                .build(),
            context: Default::default(),
        };

        let response = handle(
            lambda_event,
            &zoho_service,
            &access_token_service,
            &user_service,
        )
        .await
        .unwrap();
        assert_eq!(204, response.status_code);
    }

    #[tokio::test]
    async fn should_pass_user_id_when_authenticated() {
        let user_id = UserId::new();
        let expected_user_id = user_id;

        let mut zoho_service = MockZohoCampaignsService::default();
        zoho_service
            .expect_subscribe()
            .withf(move |sub| sub.user_id == Some(expected_user_id))
            .return_once(|_| Box::pin(async { Ok(()) }));

        let mut access_token_service = MockAccessTokenVerifierService::default();
        access_token_service
            .expect_verify_extract_user_id()
            .return_once(move |_| Box::pin(async move { Ok(Some(user_id)) }));

        let mut user_service = MockUserService::default();
        user_service.expect_find_user().return_once(move |_| {
            use fake::{Fake, Faker};
            let mut user: user::core::user::User = Faker.fake();
            user.user_id = expected_user_id;
            Box::pin(async move { Ok(user) })
        });

        let data = PutNewsletterSubscriptionData {
            email: "auth@example.com".try_into().unwrap(),
            first_name: None,
            last_name: None,
            language: None,
            currency: None,
        };

        let lambda_event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::PUT)
                .route_key("PUT /api/v1/newsletter-subscriptions")
                .body_serde(&data)
                .jwt_claim("sub", user_id)
                .build(),
            context: Default::default(),
        };

        let response = handle(
            lambda_event,
            &zoho_service,
            &access_token_service,
            &user_service,
        )
        .await
        .unwrap();
        assert_eq!(204, response.status_code);
    }

    #[tokio::test]
    async fn should_use_user_values_as_fallback_when_request_fields_are_none() {
        use common::currency::domain::Currency;
        use common::language::domain::Language;
        use user::core::first_name::FirstName;
        use user::core::last_name::LastName;

        let user_id = UserId::new();
        let expected_user_id = user_id;

        let mut zoho_service = MockZohoCampaignsService::default();
        zoho_service
            .expect_subscribe()
            .withf(move |sub| {
                sub.first_name == Some(FirstName::from("FallbackFirst"))
                    && sub.last_name == Some(LastName::from("FallbackLast"))
                    && sub.language == Some(Language::De)
                    && sub.currency == Some(Currency::Usd)
            })
            .return_once(|_| Box::pin(async { Ok(()) }));

        let mut access_token_service = MockAccessTokenVerifierService::default();
        access_token_service
            .expect_verify_extract_user_id()
            .return_once(move |_| Box::pin(async move { Ok(Some(user_id)) }));

        let mut user_service = MockUserService::default();
        user_service.expect_find_user().return_once(move |_| {
            use fake::{Fake, Faker};
            let mut user: user::core::user::User = Faker.fake();
            user.user_id = expected_user_id;
            user.first_name = Some(FirstName::from("FallbackFirst"));
            user.last_name = Some(LastName::from("FallbackLast"));
            user.language = Some(Language::De);
            user.currency = Some(Currency::Usd);
            Box::pin(async move { Ok(user) })
        });

        let data = PutNewsletterSubscriptionData {
            email: "fallback@example.com".try_into().unwrap(),
            first_name: None,
            last_name: None,
            language: None,
            currency: None,
        };

        let lambda_event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::PUT)
                .route_key("PUT /api/v1/newsletter-subscriptions")
                .body_serde(&data)
                .jwt_claim("sub", user_id)
                .build(),
            context: Default::default(),
        };

        let response = handle(
            lambda_event,
            &zoho_service,
            &access_token_service,
            &user_service,
        )
        .await
        .unwrap();
        assert_eq!(204, response.status_code);
    }

    #[tokio::test]
    async fn should_prefer_request_values_over_user_fallback() {
        use common::currency::domain::Currency;
        use common::language::domain::Language;
        use user::core::first_name::FirstName;
        use user::core::last_name::LastName;

        let user_id = UserId::new();
        let expected_user_id = user_id;

        let mut zoho_service = MockZohoCampaignsService::default();
        zoho_service
            .expect_subscribe()
            .withf(move |sub| {
                sub.first_name == Some(FirstName::from("RequestFirst"))
                    && sub.last_name == Some(LastName::from("RequestLast"))
                    && sub.language == Some(Language::En)
                    && sub.currency == Some(Currency::Eur)
            })
            .return_once(|_| Box::pin(async { Ok(()) }));

        let mut access_token_service = MockAccessTokenVerifierService::default();
        access_token_service
            .expect_verify_extract_user_id()
            .return_once(move |_| Box::pin(async move { Ok(Some(user_id)) }));

        let mut user_service = MockUserService::default();
        user_service.expect_find_user().return_once(move |_| {
            use fake::{Fake, Faker};
            let mut user: user::core::user::User = Faker.fake();
            user.user_id = expected_user_id;
            user.first_name = Some(FirstName::from("FallbackFirst"));
            user.last_name = Some(LastName::from("FallbackLast"));
            user.language = Some(Language::De);
            user.currency = Some(Currency::Usd);
            Box::pin(async move { Ok(user) })
        });

        let data = PutNewsletterSubscriptionData {
            email: "prefer@example.com".try_into().unwrap(),
            first_name: Some("RequestFirst".into()),
            last_name: Some("RequestLast".into()),
            language: Some(common::language::data::LanguageData::En),
            currency: Some(common::currency::data::CurrencyData::Eur),
        };

        let lambda_event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::PUT)
                .route_key("PUT /api/v1/newsletter-subscriptions")
                .body_serde(&data)
                .jwt_claim("sub", user_id)
                .build(),
            context: Default::default(),
        };

        let response = handle(
            lambda_event,
            &zoho_service,
            &access_token_service,
            &user_service,
        )
        .await
        .unwrap();
        assert_eq!(204, response.status_code);
    }

    #[tokio::test]
    async fn should_pass_none_user_id_when_unauthenticated() {
        let mut zoho_service = MockZohoCampaignsService::default();
        zoho_service
            .expect_subscribe()
            .withf(|sub| sub.user_id.is_none())
            .return_once(|_| Box::pin(async { Ok(()) }));

        let mut access_token_service = MockAccessTokenVerifierService::default();
        access_token_service
            .expect_verify_extract_user_id()
            .return_once(|_| Box::pin(async { Ok(None) }));

        let user_service = MockUserService::default();

        let data = PutNewsletterSubscriptionData {
            email: "anon@example.com".try_into().unwrap(),
            first_name: None,
            last_name: None,
            language: None,
            currency: None,
        };

        let lambda_event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::PUT)
                .route_key("PUT /api/v1/newsletter-subscriptions")
                .body_serde(&data)
                .build(),
            context: Default::default(),
        };

        let response = handle(
            lambda_event,
            &zoho_service,
            &access_token_service,
            &user_service,
        )
        .await
        .unwrap();
        assert_eq!(204, response.status_code);
    }

    #[tokio::test]
    async fn should_return_400_when_body_is_empty() {
        let zoho_service = MockZohoCampaignsService::default();
        let mut access_token_service = MockAccessTokenVerifierService::default();
        access_token_service
            .expect_verify_extract_user_id()
            .return_once(|_| Box::pin(async { Ok(None) }));

        let user_service = MockUserService::default();

        let lambda_event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::PUT)
                .route_key("PUT /api/v1/newsletter-subscriptions")
                .build(),
            context: Default::default(),
        };

        let response = handle(
            lambda_event,
            &zoho_service,
            &access_token_service,
            &user_service,
        )
        .await;
        assert!(response.is_err());
        assert_eq!(400, response.unwrap_err().status);
    }

    #[tokio::test]
    async fn should_return_400_when_body_is_invalid_json() {
        let zoho_service = MockZohoCampaignsService::default();
        let mut access_token_service = MockAccessTokenVerifierService::default();
        access_token_service
            .expect_verify_extract_user_id()
            .return_once(|_| Box::pin(async { Ok(None) }));

        let user_service = MockUserService::default();

        let lambda_event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::PUT)
                .route_key("PUT /api/v1/newsletter-subscriptions")
                .body_serde(&"not a valid newsletter body")
                .build(),
            context: Default::default(),
        };

        let response = handle(
            lambda_event,
            &zoho_service,
            &access_token_service,
            &user_service,
        )
        .await;
        assert!(response.is_err());
        assert_eq!(400, response.unwrap_err().status);
    }

    #[tokio::test]
    async fn should_return_500_when_zoho_service_fails_with_server_error_code() {
        use crate::service::ZohoCampaignsError;

        let mut zoho_service = MockZohoCampaignsService::default();
        zoho_service.expect_subscribe().return_once(|_| {
            Box::pin(async {
                Err(ZohoCampaignsError::ApiResponseError {
                    status: "error".to_string(),
                    message: Some("Listkey is empty or invalid.".to_string()),
                    code: Some(2501),
                })
            })
        });

        let mut access_token_service = MockAccessTokenVerifierService::default();
        access_token_service
            .expect_verify_extract_user_id()
            .return_once(|_| Box::pin(async { Ok(None) }));

        let user_service = MockUserService::default();

        let data = PutNewsletterSubscriptionData {
            email: "fail@example.com".try_into().unwrap(),
            first_name: None,
            last_name: None,
            language: None,
            currency: None,
        };

        let lambda_event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::PUT)
                .route_key("PUT /api/v1/newsletter-subscriptions")
                .body_serde(&data)
                .build(),
            context: Default::default(),
        };

        let response = handle(
            lambda_event,
            &zoho_service,
            &access_token_service,
            &user_service,
        )
        .await;
        assert!(response.is_err());
        assert_eq!(500, response.unwrap_err().status);
    }

    #[tokio::test]
    async fn should_return_400_when_zoho_returns_invalid_email_code_2004() {
        use crate::service::ZohoCampaignsError;

        let mut zoho_service = MockZohoCampaignsService::default();
        zoho_service.expect_subscribe().return_once(|_| {
            Box::pin(async {
                Err(ZohoCampaignsError::ApiResponseError {
                    status: "error".to_string(),
                    message: Some("Invalid contact email address.".to_string()),
                    code: Some(2004),
                })
            })
        });

        let mut access_token_service = MockAccessTokenVerifierService::default();
        access_token_service
            .expect_verify_extract_user_id()
            .return_once(|_| Box::pin(async { Ok(None) }));

        let user_service = MockUserService::default();

        let data = PutNewsletterSubscriptionData {
            email: "invalid@example.com".try_into().unwrap(),
            first_name: None,
            last_name: None,
            language: None,
            currency: None,
        };

        let lambda_event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::PUT)
                .route_key("PUT /api/v1/newsletter-subscriptions")
                .body_serde(&data)
                .build(),
            context: Default::default(),
        };

        let response = handle(
            lambda_event,
            &zoho_service,
            &access_token_service,
            &user_service,
        )
        .await;
        assert!(response.is_err());
        let err = response.unwrap_err();
        assert_eq!(400, err.status);
        assert_eq!("INVALID_EMAIL", err.error.as_str());
    }

    #[tokio::test]
    async fn should_return_400_when_zoho_returns_group_email_code_2005() {
        use crate::service::ZohoCampaignsError;

        let mut zoho_service = MockZohoCampaignsService::default();
        zoho_service.expect_subscribe().return_once(|_| {
            Box::pin(async {
                Err(ZohoCampaignsError::ApiResponseError {
                    status: "error".to_string(),
                    message: Some("Group email address added.".to_string()),
                    code: Some(2005),
                })
            })
        });

        let mut access_token_service = MockAccessTokenVerifierService::default();
        access_token_service
            .expect_verify_extract_user_id()
            .return_once(|_| Box::pin(async { Ok(None) }));

        let user_service = MockUserService::default();

        let data = PutNewsletterSubscriptionData {
            email: "group@example.com".try_into().unwrap(),
            first_name: None,
            last_name: None,
            language: None,
            currency: None,
        };

        let lambda_event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::PUT)
                .route_key("PUT /api/v1/newsletter-subscriptions")
                .body_serde(&data)
                .build(),
            context: Default::default(),
        };

        let response = handle(
            lambda_event,
            &zoho_service,
            &access_token_service,
            &user_service,
        )
        .await;
        assert!(response.is_err());
        let err = response.unwrap_err();
        assert_eq!(400, err.status);
        assert_eq!("INVALID_EMAIL", err.error.as_str());
    }
}
