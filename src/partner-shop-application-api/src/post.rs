use aws_lambda_events::apigw::{ApiGatewayV2httpRequest, ApiGatewayV2httpResponse};
use common::actor::{RequestContext, domain::Actor};
use common::api::api_gateway_v2_http_response_builder::ApiGatewayV2HttpResponseBuilder;
use common::api::error::ApiError;
use common::api::error_code::BAD_BODY_VALUE;
use common::user_id::api::extract_user_id_request_context;
use lambda_runtime::LambdaEvent;
use partner_shop_application::core::command::{
    CreatePartnerShopApplicationCommand, CreatePartnerShopApplicationPayload,
};
use partner_shop_application::data::get_partner_shop_application_data::GetPartnerShopApplicationData;
use partner_shop_application::data::post_partner_shop_application_data::PostPartnerShopApplicationPayloadData;
use partner_shop_application::service::partner_shop_application_service::PartnerShopApplicationService;
use shop::core::partner_status::ShopPartnerStatus;
use shop::service::command::CreateShopCommand;

pub async fn handle(
    event: LambdaEvent<ApiGatewayV2httpRequest>,
    service: &impl PartnerShopApplicationService,
) -> Result<ApiGatewayV2httpResponse, ApiError> {
    let user_id = extract_user_id_request_context(&event.payload.request_context)?;
    tracing::Span::current().record("userId", user_id.to_string());

    let body = event
        .payload
        .body
        .filter(|str| !str.is_empty())
        .ok_or_else(|| {
            let err_msg = "Body cannot be empty";
            ApiError::bad_request(BAD_BODY_VALUE, err_msg.into()).with_detail(err_msg)
        })?;
    let post_data: PostPartnerShopApplicationPayloadData =
        serde_json::from_str(&body).map_err(|err| {
            let err_msg = err.to_string();
            ApiError::bad_request(BAD_BODY_VALUE, Box::new(err)).with_detail(err_msg)
        })?;

    let payload = match post_data {
        PostPartnerShopApplicationPayloadData::Existing { shop_id } => {
            CreatePartnerShopApplicationPayload::Existing(shop_id)
        }
        PostPartnerShopApplicationPayloadData::New {
            shop_name,
            shop_type,
            shop_domains,
            shop_url,
            shop_image,
            shop_structured_address,
            shop_phone,
            shop_email,
        } => CreatePartnerShopApplicationPayload::New(CreateShopCommand {
            name: shop_name,
            shop_type: shop_type.into(),
            shop_partner_status: ShopPartnerStatus::Partnered,
            domains: shop_domains,
            shopify_domain: None,
            shopify_currency: None,
            shopify_language: None,
            woocommerce_webhook_secret: None,
            woocommerce_currency: None,
            woocommerce_language: None,
            url: shop_url,
            image: shop_image,
            structured_address: shop_structured_address.map(Into::into),
            phone: shop_phone,
            email: shop_email,
            affiliate_configuration: None,
        }),
    };

    let cmd = CreatePartnerShopApplicationCommand {
        applicant_user_id: user_id,
        payload,
    };

    let data: GetPartnerShopApplicationData = service
        .create_partner_shop_application(
            &RequestContext {
                actor: Actor::User(user_id),
            },
            cmd,
        )
        .await?
        .into();

    Ok(ApiGatewayV2HttpResponseBuilder::json(201)
        .last_modified(data.updated)
        .body_serde(data)?
        .build())
}

#[cfg(test)]
mod tests {
    use crate::handle;
    use common::user_id::UserId;
    use fake::{Fake, Faker};
    use lambda_runtime::LambdaEvent;
    use partner_shop_application::{
        core::partner_shop_application::PartnerShopApplication,
        data::post_partner_shop_application_data::PostPartnerShopApplicationPayloadData,
        service::partner_shop_application_service::MockPartnerShopApplicationService,
    };
    use test_api::ApiGatewayV2httpRequestProxy;
    use user::service::user_service::MockUserService;

    #[tokio::test]
    async fn should_201_when_creating_application() {
        let mut service = MockPartnerShopApplicationService::default();
        service
            .expect_create_partner_shop_application()
            .return_once(move |_, _| {
                let app: PartnerShopApplication = Faker.fake();
                Box::pin(async move { Ok(app) })
            });
        let user_service = MockUserService::default();
        let lambda_event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::POST)
                .route_key("POST /api/v1/me/partner-applications")
                .jwt_claim("sub", UserId::new())
                .body_serde(&Faker.fake::<PostPartnerShopApplicationPayloadData>())
                .build(),
            context: Default::default(),
        };

        let response = handle(lambda_event, &service, &user_service).await.unwrap();
        assert_eq!(201, response.status_code);
    }

    #[tokio::test]
    async fn should_401_when_jwt_claim_sub_is_missing() {
        let service = MockPartnerShopApplicationService::default();
        let user_service = MockUserService::default();
        let lambda_event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::POST)
                .route_key("POST /api/v1/me/partner-applications")
                .body_serde(&Faker.fake::<PostPartnerShopApplicationPayloadData>())
                .build(),
            context: Default::default(),
        };

        let response = handle(lambda_event, &service, &user_service)
            .await
            .unwrap_err();
        assert_eq!(401, response.status);
    }

    #[tokio::test]
    async fn should_400_when_body_is_empty() {
        let service = MockPartnerShopApplicationService::default();
        let user_service = MockUserService::default();
        let lambda_event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::POST)
                .route_key("POST /api/v1/me/partner-applications")
                .jwt_claim("sub", UserId::new())
                .build(),
            context: Default::default(),
        };

        let response = handle(lambda_event, &service, &user_service)
            .await
            .unwrap_err();
        assert_eq!(400, response.status);
    }

    #[tokio::test]
    async fn should_400_when_body_is_invalid_json() {
        let service = MockPartnerShopApplicationService::default();
        let user_service = MockUserService::default();
        let lambda_event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::POST)
                .route_key("POST /api/v1/me/partner-applications")
                .jwt_claim("sub", UserId::new())
                .body_serde(&"not valid json structure")
                .build(),
            context: Default::default(),
        };

        let response = handle(lambda_event, &service, &user_service)
            .await
            .unwrap_err();
        assert_eq!(400, response.status);
    }
}
