use crate::billing::BillingRequest;
use crate::service::{CreateStripeCustomerCommand, StripeService};
use aws_lambda_events::apigw::{ApiGatewayV2httpRequest, ApiGatewayV2httpResponse};
use common::actor::{RequestContext, domain::Actor};
use common::api::api_gateway_v2_http_response_builder::ApiGatewayV2HttpResponseBuilder;
use common::api::error::ApiError;
use common::api::error_code::{
    BAD_BODY_VALUE, INTERNAL_SERVER_ERROR, STRIPE_CUSTOMER_DOES_NOT_EXIST,
};
use common::user_id::api::extract_user_id_request_context;
use lambda_runtime::LambdaEvent;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use url::Url;
use user::core::tier::UserTier;
use user::service::command::UpdateUserCommand;
use user::service::user_service::UserService;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManageBillingRequest {
    #[serde(flatten)]
    pub billing_request: BillingRequest,
}

#[derive(Debug, Serialize)]
pub struct ManageBillingResponse {
    pub url: Url,
}

pub async fn handle(
    event: LambdaEvent<ApiGatewayV2httpRequest>,
    stripe_service: &impl StripeService,
    user_service: &(impl UserService + Sync),
    price_ids: &HashMap<&'static str, String>,
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
    let billing_request: ManageBillingRequest = serde_json::from_str(&body).map_err(|err| {
        let err_msg = err.to_string();
        ApiError::bad_request(BAD_BODY_VALUE, Box::new(err)).with_detail(err_msg)
    })?;

    let user = user_service.find_user(&user_id).await?;

    let url = match user.tier {
        UserTier::Free => {
            let env_var = billing_request.billing_request.price_id_env_var();
            let price_id = price_ids.get(env_var).ok_or_else(|| {
                let err_msg = format!("Missing configured price-id for env-var '{env_var}'");
                ApiError::internal_server_error(INTERNAL_SERVER_ERROR, err_msg.into())
            })?;

            let stripe_customer_id = match user.stripe_customer_id.clone() {
                Some(stripe_customer_id) => stripe_customer_id,
                None => {
                    let create_customer = CreateStripeCustomerCommand {
                        user_id,
                        email: user.email.clone(),
                        name: user.name(),
                    };
                    let stripe_customer_id = stripe_service
                        .create_customer(&create_customer)
                        .await
                        .map_err(|err| {
                            ApiError::internal_server_error(INTERNAL_SERVER_ERROR, Box::new(err))
                        })?;

                    user_service
                        .update_user(
                            &RequestContext {
                                actor: Actor::User(user_id),
                            },
                            &user_id,
                            UpdateUserCommand {
                                stripe_customer_id: Some(stripe_customer_id.clone()),
                                ..Default::default()
                            },
                        )
                        .await?;

                    stripe_customer_id
                }
            };

            stripe_service
                .create_checkout_session(&user_id, &stripe_customer_id, price_id)
                .await
                .map_err(|err| {
                    ApiError::internal_server_error(INTERNAL_SERVER_ERROR, Box::new(err))
                })?
        }
        UserTier::Pro | UserTier::Ultimate => {
            let stripe_customer_id = user.stripe_customer_id.as_ref().ok_or_else(|| {
                let err_msg = "User has never had a Stripe subscription; no customer-portal session can be created";
                ApiError::unprocessable_entity(STRIPE_CUSTOMER_DOES_NOT_EXIST, err_msg.into())
                    .with_detail(err_msg)
            })?;

            stripe_service
                .create_portal_session(stripe_customer_id)
                .await
                .map_err(|err| {
                    ApiError::internal_server_error(INTERNAL_SERVER_ERROR, Box::new(err))
                })?
        }
    };

    Ok(ApiGatewayV2HttpResponseBuilder::json(201)
        .body_serde(ManageBillingResponse { url })?
        .build())
}

#[cfg(test)]
mod tests {
    use super::{ManageBillingRequest, handle};
    use crate::billing::{BillingCycle, BillingPlan, BillingRequest};
    use crate::service::MockStripeService;
    use common::api::error_code::STRIPE_CUSTOMER_DOES_NOT_EXIST;
    use common::stripe_customer_id::StripeCustomerId;
    use common::user_id::UserId;
    use fake::{Fake, Faker};
    use lambda_runtime::LambdaEvent;
    use rstest::rstest;
    use std::collections::HashMap;
    use test_api::ApiGatewayV2httpRequestProxy;
    use url::Url;
    use user::core::{tier::UserTier, user::User};
    use user::service::user_service::MockUserService;

    fn user_with_tier(tier: UserTier, stripe_customer_id: Option<&str>) -> User {
        let mut user: User = Faker.fake();
        user.tier = tier;
        user.stripe_customer_id = stripe_customer_id.map(StripeCustomerId::from);
        user.first_name = Some("Ada".into());
        user.last_name = Some("Lovelace".into());
        user
    }

    fn body_for(plan: BillingPlan, cycle: BillingCycle) -> ManageBillingRequest {
        ManageBillingRequest {
            billing_request: BillingRequest { plan, cycle },
        }
    }

    fn price_ids() -> HashMap<&'static str, String> {
        HashMap::from([
            ("STRIPE_PRO_MONTHLY_PRICE_ID", "price_pro_m".to_owned()),
            ("STRIPE_PRO_YEARLY_PRICE_ID", "price_pro_y".to_owned()),
            ("STRIPE_ULTIMATE_MONTHLY_PRICE_ID", "price_ult_m".to_owned()),
            ("STRIPE_ULTIMATE_YEARLY_PRICE_ID", "price_ult_y".to_owned()),
        ])
    }

    #[tokio::test]
    async fn should_201_with_checkout_url_when_user_is_free_and_has_no_stripe_customer_id_for_manage()
     {
        let mut user_service = MockUserService::default();
        user_service
            .expect_find_user()
            .return_once(|_| Box::pin(async move { Ok(user_with_tier(UserTier::Free, None)) }));
        user_service.expect_update_user().return_once(|_, _| {
            Box::pin(async move { Ok(user_with_tier(UserTier::Free, Some("cus_freshly_created"))) })
        });

        let mut stripe_service = MockStripeService::default();
        let created_customer_id = StripeCustomerId::from("cus_freshly_created");
        let created_customer_id_clone = created_customer_id.clone();
        stripe_service
            .expect_create_customer()
            .return_once(move |req| {
                assert_eq!(req.name.as_deref(), Some("Ada Lovelace"));
                Box::pin(async move { Ok(created_customer_id_clone) })
            });
        stripe_service
            .expect_create_checkout_session()
            .withf(|_, customer_id, price_id| {
                customer_id.as_ref() == "cus_freshly_created" && price_id == "price_pro_m"
            })
            .return_once(|_, _, _| {
                Box::pin(async move {
                    Ok(
                        Url::parse("https://checkout.stripe.com/c/pay/cs_test_manage_free")
                            .unwrap(),
                    )
                })
            });
        stripe_service.expect_create_portal_session().never();

        let lambda_event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::POST)
                .jwt_claim("sub", UserId::new())
                .body_serde(&body_for(BillingPlan::Pro, BillingCycle::Monthly))
                .build(),
            context: Default::default(),
        };

        let response = handle(lambda_event, &stripe_service, &user_service, &price_ids())
            .await
            .unwrap();

        assert_eq!(201, response.status_code);
        let body = match response.body.unwrap() {
            aws_lambda_events::encodings::Body::Text(s) => s,
            _ => panic!("expected text body"),
        };
        assert!(body.contains("checkout.stripe.com"));
        assert!(!body.contains("livemode"));
        assert!(!body.contains("userId"));
    }

    #[tokio::test]
    async fn should_201_with_checkout_url_when_user_is_free_and_has_existing_stripe_customer_id_for_manage()
     {
        let mut user_service = MockUserService::default();
        user_service.expect_find_user().return_once(|_| {
            Box::pin(async move { Ok(user_with_tier(UserTier::Free, Some("cus_existing"))) })
        });
        user_service.expect_update_user().never();

        let mut stripe_service = MockStripeService::default();
        stripe_service.expect_create_customer().never();
        stripe_service
            .expect_create_checkout_session()
            .withf(|_, customer_id, price_id| {
                customer_id.as_ref() == "cus_existing" && price_id == "price_ult_y"
            })
            .return_once(|_, _, _| {
                Box::pin(async move {
                    Ok(
                        Url::parse("https://checkout.stripe.com/c/pay/cs_test_manage_existing")
                            .unwrap(),
                    )
                })
            });
        stripe_service.expect_create_portal_session().never();

        let lambda_event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::POST)
                .jwt_claim("sub", UserId::new())
                .body_serde(&body_for(BillingPlan::Ultimate, BillingCycle::Yearly))
                .build(),
            context: Default::default(),
        };

        let response = handle(lambda_event, &stripe_service, &user_service, &price_ids())
            .await
            .unwrap();

        assert_eq!(201, response.status_code);
        let body = match response.body.unwrap() {
            aws_lambda_events::encodings::Body::Text(s) => s,
            _ => panic!("expected text body"),
        };
        assert!(body.contains("checkout.stripe.com"));
        assert!(!body.contains("livemode"));
        assert!(!body.contains("userId"));
    }

    #[rstest]
    #[case(UserTier::Pro, "cus_pro")]
    #[case(UserTier::Ultimate, "cus_ultimate")]
    #[tokio::test]
    async fn should_201_with_portal_url_when_user_is_paid_and_has_stripe_customer_id_for_manage(
        #[case] tier: UserTier,
        #[case] stripe_customer_id: &'static str,
    ) {
        let mut user_service = MockUserService::default();
        user_service.expect_find_user().return_once(move |_| {
            Box::pin(async move { Ok(user_with_tier(tier, Some(stripe_customer_id))) })
        });
        user_service.expect_update_user().never();

        let mut stripe_service = MockStripeService::default();
        stripe_service.expect_create_customer().never();
        stripe_service.expect_create_checkout_session().never();
        stripe_service
            .expect_create_portal_session()
            .withf(move |customer_id| customer_id.as_ref() == stripe_customer_id)
            .return_once(|_| {
                Box::pin(async move {
                    Ok(Url::parse("https://billing.stripe.com/p/session/manage_paid").unwrap())
                })
            });

        let lambda_event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::POST)
                .jwt_claim("sub", UserId::new())
                .body_serde(&body_for(BillingPlan::Pro, BillingCycle::Monthly))
                .build(),
            context: Default::default(),
        };

        let response = handle(
            lambda_event,
            &stripe_service,
            &user_service,
            &HashMap::new(),
        )
        .await
        .unwrap();

        assert_eq!(201, response.status_code);
        let body = match response.body.unwrap() {
            aws_lambda_events::encodings::Body::Text(s) => s,
            _ => panic!("expected text body"),
        };
        assert!(body.contains("billing.stripe.com"));
        assert!(!body.contains("livemode"));
        assert!(!body.contains("userId"));
    }

    #[rstest]
    #[case(UserTier::Pro)]
    #[case(UserTier::Ultimate)]
    #[tokio::test]
    async fn should_422_with_dedicated_error_code_when_paid_user_has_no_stripe_customer_id_for_manage(
        #[case] tier: UserTier,
    ) {
        let mut user_service = MockUserService::default();
        user_service
            .expect_find_user()
            .return_once(move |_| Box::pin(async move { Ok(user_with_tier(tier, None)) }));

        let mut stripe_service = MockStripeService::default();
        stripe_service.expect_create_customer().never();
        stripe_service.expect_create_checkout_session().never();
        stripe_service.expect_create_portal_session().never();

        let lambda_event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::POST)
                .jwt_claim("sub", UserId::new())
                .body_serde(&body_for(BillingPlan::Ultimate, BillingCycle::Monthly))
                .build(),
            context: Default::default(),
        };

        let actual = handle(
            lambda_event,
            &stripe_service,
            &user_service,
            &HashMap::new(),
        )
        .await
        .unwrap_err();

        assert_eq!(422, actual.status);
        assert_eq!(STRIPE_CUSTOMER_DOES_NOT_EXIST, actual.error);
    }

    #[tokio::test]
    async fn should_400_when_body_is_missing_for_manage() {
        let user_service = MockUserService::default();
        let stripe_service = MockStripeService::default();

        let lambda_event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::POST)
                .jwt_claim("sub", UserId::new())
                .build(),
            context: Default::default(),
        };

        let actual = handle(lambda_event, &stripe_service, &user_service, &price_ids())
            .await
            .unwrap_err();

        assert_eq!(400, actual.status);
    }

    #[tokio::test]
    async fn should_400_when_body_has_unknown_plan_for_manage() {
        let user_service = MockUserService::default();
        let stripe_service = MockStripeService::default();

        let lambda_event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::POST)
                .jwt_claim("sub", UserId::new())
                .body_serde(&serde_json::json!({"plan":"ENTERPRISE","cycle":"MONTHLY"}))
                .build(),
            context: Default::default(),
        };

        let actual = handle(lambda_event, &stripe_service, &user_service, &price_ids())
            .await
            .unwrap_err();

        assert_eq!(400, actual.status);
    }

    #[tokio::test]
    async fn should_401_when_sub_missing_for_manage() {
        let user_service = MockUserService::default();
        let mut stripe_service = MockStripeService::default();
        stripe_service.expect_create_customer().never();
        stripe_service.expect_create_checkout_session().never();
        stripe_service.expect_create_portal_session().never();

        let lambda_event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::POST)
                .body_serde(&body_for(BillingPlan::Pro, BillingCycle::Monthly))
                .build(),
            context: Default::default(),
        };

        let actual = handle(lambda_event, &stripe_service, &user_service, &price_ids())
            .await
            .unwrap_err();

        assert_eq!(401, actual.status);
    }

    #[tokio::test]
    async fn should_500_when_price_id_env_var_not_configured_for_free_manage() {
        let mut user_service = MockUserService::default();
        user_service.expect_find_user().return_once(|_| {
            Box::pin(async move { Ok(user_with_tier(UserTier::Free, Some("cus_existing"))) })
        });

        let mut stripe_service = MockStripeService::default();
        stripe_service.expect_create_customer().never();
        stripe_service.expect_create_checkout_session().never();
        stripe_service.expect_create_portal_session().never();

        let lambda_event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::POST)
                .jwt_claim("sub", UserId::new())
                .body_serde(&body_for(BillingPlan::Ultimate, BillingCycle::Yearly))
                .build(),
            context: Default::default(),
        };

        let mut incomplete = price_ids();
        incomplete.remove("STRIPE_ULTIMATE_YEARLY_PRICE_ID");

        let actual = handle(lambda_event, &stripe_service, &user_service, &incomplete)
            .await
            .unwrap_err();

        assert_eq!(500, actual.status);
    }
}
