use application::error::box_error;
use billing_service::ports::{
    CreateStripeCheckoutSessionRequest, CreateStripeCustomerRequest,
    CreateStripePortalSessionRequest, StripeBillingError, StripeCheckoutSessionCreator,
    StripeCustomerCreator, StripePortalSessionCreator,
};
use reqwest::StatusCode;
use serde::Deserialize;
use std::time::Duration;
use url::Url;
use user_core::stripe_customer_id::StripeCustomerId;

const STRIPE_API_BASE_URL: &str = "https://api.stripe.com";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StripeBillingConfig {
    pub api_key: String,
    pub checkout_success_url: Url,
    pub checkout_cancel_url: Url,
    pub portal_return_url: Url,
}

#[derive(Clone)]
pub struct StripeBillingClient {
    client: reqwest::Client,
    config: StripeBillingConfig,
    api_base_url: Url,
}

#[derive(Debug, thiserror::Error)]
pub enum StripeBillingClientInitError {
    #[error("failed to construct Stripe HTTP client")]
    HttpClient(#[source] reqwest::Error),
    #[error("invalid built-in Stripe API base URL")]
    ApiBaseUrl(#[source] url::ParseError),
}

impl StripeBillingClient {
    pub fn new(config: StripeBillingConfig) -> Result<Self, StripeBillingClientInitError> {
        let client = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(2))
            .timeout(Duration::from_secs(10))
            .build()
            .map_err(StripeBillingClientInitError::HttpClient)?;
        let api_base_url =
            Url::parse(STRIPE_API_BASE_URL).map_err(StripeBillingClientInitError::ApiBaseUrl)?;
        Ok(Self::with_client(client, config, api_base_url))
    }

    pub fn with_client(
        client: reqwest::Client,
        config: StripeBillingConfig,
        api_base_url: Url,
    ) -> Self {
        Self {
            client,
            config,
            api_base_url,
        }
    }

    async fn post_form(
        &self,
        path: &str,
        form: Vec<(&str, String)>,
        idempotency_key: Option<String>,
    ) -> Result<serde_json::Value, StripeBillingError> {
        let url =
            self.api_base_url
                .join(path)
                .map_err(|error| StripeBillingError::InvalidResponse {
                    source: box_error(error),
                })?;
        let mut request = self
            .client
            .post(url)
            .bearer_auth(&self.config.api_key)
            .form(&form);
        if let Some(idempotency_key) = idempotency_key {
            request = request.header("Idempotency-Key", idempotency_key);
        }
        let response =
            request
                .send()
                .await
                .map_err(|error| StripeBillingError::TemporarilyUnavailable {
                    source: box_error(error),
                })?;
        let status = response.status();
        if !status.is_success() {
            return Err(classify_status(status));
        }
        response
            .json()
            .await
            .map_err(|error| StripeBillingError::InvalidResponse {
                source: box_error(error),
            })
    }
}

#[async_trait::async_trait]
impl StripeCustomerCreator for StripeBillingClient {
    async fn create_customer(
        &self,
        request: CreateStripeCustomerRequest,
    ) -> Result<StripeCustomerId, StripeBillingError> {
        let mut form = vec![
            ("email", request.email.to_string()),
            ("metadata[userId]", request.user_id.to_string()),
        ];
        if let Some(name) = request.name {
            form.push(("name", name));
        }
        let body = self
            .post_form(
                "v1/customers",
                form,
                request
                    .idempotency_key
                    .map(|key| format!("billing-customer:{}:{key}", request.user_id)),
            )
            .await?;
        StripeCustomerResponse::from_value(body).map(|response| StripeCustomerId::from(response.id))
    }
}

#[async_trait::async_trait]
impl StripeCheckoutSessionCreator for StripeBillingClient {
    async fn create_checkout_session(
        &self,
        request: CreateStripeCheckoutSessionRequest,
    ) -> Result<Url, StripeBillingError> {
        let form = vec![
            ("mode", "subscription".to_owned()),
            ("customer", request.stripe_customer_id.to_string()),
            ("customer_update[address]", "auto".to_owned()),
            ("customer_update[name]", "auto".to_owned()),
            ("line_items[0][price]", request.price_id),
            ("line_items[0][quantity]", "1".to_owned()),
            ("billing_address_collection", "required".to_owned()),
            ("automatic_tax[enabled]", "true".to_owned()),
            ("tax_id_collection[enabled]", "true".to_owned()),
            ("custom_fields[0][key]", "business_name".to_owned()),
            ("custom_fields[0][type]", "text".to_owned()),
            ("custom_fields[0][label][type]", "custom".to_owned()),
            (
                "custom_fields[0][label][custom]",
                "Business name".to_owned(),
            ),
            ("custom_fields[0][optional]", "true".to_owned()),
            ("client_reference_id", request.user_id.to_string()),
            (
                "subscription_data[metadata][userId]",
                request.user_id.to_string(),
            ),
            ("success_url", self.config.checkout_success_url.to_string()),
            ("cancel_url", self.config.checkout_cancel_url.to_string()),
        ];
        let customer_id = request.stripe_customer_id.to_string();
        let body = self
            .post_form(
                "v1/checkout/sessions",
                form,
                request
                    .idempotency_key
                    .map(|key| format!("billing-checkout:{customer_id}:{key}")),
            )
            .await?;
        StripeSessionResponse::from_value(body)?.url()
    }
}

#[async_trait::async_trait]
impl StripePortalSessionCreator for StripeBillingClient {
    async fn create_portal_session(
        &self,
        request: CreateStripePortalSessionRequest,
    ) -> Result<Url, StripeBillingError> {
        let customer_id = request.stripe_customer_id.to_string();
        let body = self
            .post_form(
                "v1/billing_portal/sessions",
                vec![
                    ("customer", customer_id.clone()),
                    ("return_url", self.config.portal_return_url.to_string()),
                ],
                request
                    .idempotency_key
                    .map(|key| format!("billing-portal:{customer_id}:{key}")),
            )
            .await?;
        StripeSessionResponse::from_value(body)?.url()
    }
}

#[derive(Deserialize)]
struct StripeCustomerResponse {
    id: String,
}
impl StripeCustomerResponse {
    fn from_value(value: serde_json::Value) -> Result<Self, StripeBillingError> {
        serde_json::from_value(value).map_err(|error| StripeBillingError::InvalidResponse {
            source: box_error(error),
        })
    }
}
#[derive(Deserialize)]
struct StripeSessionResponse {
    url: String,
}
impl StripeSessionResponse {
    fn from_value(value: serde_json::Value) -> Result<Self, StripeBillingError> {
        serde_json::from_value(value).map_err(|error| StripeBillingError::InvalidResponse {
            source: box_error(error),
        })
    }
    fn url(self) -> Result<Url, StripeBillingError> {
        Url::parse(&self.url).map_err(|error| StripeBillingError::InvalidResponse {
            source: box_error(error),
        })
    }
}

fn classify_status(status: StatusCode) -> StripeBillingError {
    let source = box_error(std::io::Error::other(format!(
        "Stripe returned HTTP {status}"
    )));
    if status.is_server_error() || status == StatusCode::TOO_MANY_REQUESTS {
        StripeBillingError::TemporarilyUnavailable { source }
    } else {
        StripeBillingError::Rejected { source }
    }
}
