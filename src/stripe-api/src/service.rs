//! Abstraction over the Stripe HTTP API for the operations performed by the
//! `stripe-api` Lambda.
//!
//! The trait keeps the Lambda handlers fully unit-testable (via
//! [`MockStripeService`]) and lets the Lambda's `main.rs` substitute a
//! deterministic mock when running inside LocalStack.
//!
//! The production implementation, [`StripeServiceImpl`], talks to Stripe over
//! HTTPS with `reqwest` using the standard `application/x-www-form-urlencoded`
//! body shape documented at <https://stripe.com/docs/api>.

use async_trait::async_trait;
use common::{stripe_customer_id::StripeCustomerId, user_id::UserId};
use thiserror::Error;
use url::Url;

#[derive(Debug, Error)]
pub enum StripeServiceError {
    #[error("Failed performing Stripe HTTP-request: {0}")]
    Http(#[from] reqwest::Error),

    #[error("Stripe responded with status {status}: {body}")]
    Api { status: u16, body: String },

    #[error("Stripe response is missing the expected 'url' field.")]
    MissingUrl,
}

/// Subset of the Stripe API consumed by `stripe-api`.
#[async_trait]
#[mockall::automock]
pub trait StripeService: Send + Sync {
    /// Create a new Stripe Checkout-Session for the given user and return the
    /// hosted Checkout URL.
    ///
    /// `livemode` is enforced server-side by Stripe based on the supplied API
    /// key but is also surfaced through the [`crate::CheckoutSessionResponse`]
    /// payload so the frontend can sanity-check it.
    async fn create_checkout_session(&self, user_id: &UserId) -> Result<Url, StripeServiceError>;

    /// Create a new Stripe Billing-Portal-Session for the given customer and
    /// return the hosted Portal URL.
    async fn create_portal_session(
        &self,
        user_id: &UserId,
        stripe_customer_id: &StripeCustomerId,
    ) -> Result<Url, StripeServiceError>;
}

/// Production implementation calling the Stripe REST API.
///
/// All endpoints accept `application/x-www-form-urlencoded` bodies and return
/// JSON, see <https://stripe.com/docs/api/checkout/sessions/create> and
/// <https://stripe.com/docs/api/customer_portal/sessions/create>.
pub struct StripeServiceImpl {
    client: reqwest::Client,
    api_key: String,
    api_base_url: String,
    /// Stripe `Price` id (`price_…`) used as the single line-item of every
    /// Checkout-Session created by this service.
    price_id: String,
    /// Base URL of the user-facing frontend, used to construct
    /// `success_url`, `cancel_url`, and `return_url`.
    frontend_base_url: String,
}

impl StripeServiceImpl {
    pub fn new(api_key: String, price_id: String, frontend_base_url: String) -> Self {
        Self {
            client: reqwest::Client::new(),
            api_key,
            api_base_url: "https://api.stripe.com".to_owned(),
            price_id,
            frontend_base_url,
        }
    }

    async fn post_form(
        &self,
        path: &str,
        form: &[(&str, String)],
    ) -> Result<Url, StripeServiceError> {
        let url = format!("{}{}", self.api_base_url, path);
        let response = self
            .client
            .post(url)
            .bearer_auth(&self.api_key)
            .form(form)
            .send()
            .await?;

        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            return Err(StripeServiceError::Api {
                status: status.as_u16(),
                body,
            });
        }

        let body: serde_json::Value = response.json().await?;
        body.get("url")
            .and_then(|v| v.as_str())
            .and_then(|s| Url::parse(s).ok())
            .ok_or(StripeServiceError::MissingUrl)
    }
}

#[async_trait]
impl StripeService for StripeServiceImpl {
    async fn create_checkout_session(&self, user_id: &UserId) -> Result<Url, StripeServiceError> {
        let form = vec![
            ("mode", "subscription".to_owned()),
            ("line_items[0][price]", self.price_id.clone()),
            ("line_items[0][quantity]", "1".to_owned()),
            ("metadata[userId]", user_id.to_string()),
            ("subscription_data[metadata][userId]", user_id.to_string()),
            ("client_reference_id", user_id.to_string()),
            (
                "success_url",
                format!("{}/billing/success", self.frontend_base_url),
            ),
            (
                "cancel_url",
                format!("{}/billing/cancel", self.frontend_base_url),
            ),
        ];

        self.post_form("/v1/checkout/sessions", &form).await
    }

    async fn create_portal_session(
        &self,
        user_id: &UserId,
        stripe_customer_id: &StripeCustomerId,
    ) -> Result<Url, StripeServiceError> {
        let form = vec![
            ("customer", stripe_customer_id.to_string()),
            ("metadata[userId]", user_id.to_string()),
            ("return_url", format!("{}/billing", self.frontend_base_url)),
        ];

        self.post_form("/v1/billing_portal/sessions", &form).await
    }
}
