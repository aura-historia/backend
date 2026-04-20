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
use serde_email::Email;
use thiserror::Error;
use url::Url;
use user::core::name::Name;

#[derive(Debug, Error)]
pub enum StripeServiceError {
    #[error("Failed performing Stripe HTTP-request: {0}")]
    Http(#[from] reqwest::Error),

    #[error("Stripe responded with status {status}: {body}")]
    Api { status: u16, body: String },

    #[error("Stripe response is missing the expected '{0}' field.")]
    MissingField(&'static str),
}

/// Strongly-typed view of the data we forward to Stripe when creating a
/// `Customer`. Using a struct (instead of separate parameters) keeps the
/// trait signature stable as we add more fields.
#[derive(Debug, Clone, PartialEq)]
pub struct CreateStripeCustomerCommand {
    pub user_id: UserId,
    pub email: Email,
    pub name: Option<Name>,
}

/// Subset of the Stripe API consumed by `stripe-api`.
#[async_trait]
#[mockall::automock]
pub trait StripeService: Send + Sync {
    /// Create a Stripe `Customer` for the given user and return the resulting
    /// `cus_…` id. Called from the checkout endpoint when the user does not
    /// yet have a `stripe_customer_id` so that subsequent checkouts and the
    /// customer-portal can re-use it.
    async fn create_customer(
        &self,
        customer: &CreateStripeCustomerCommand,
    ) -> Result<StripeCustomerId, StripeServiceError>;

    /// Create a new Stripe Checkout-Session for an *existing* Stripe customer
    /// and return the hosted Checkout URL.
    async fn create_checkout_session(
        &self,
        user_id: &UserId,
        stripe_customer_id: &StripeCustomerId,
        price_id: &str,
    ) -> Result<Url, StripeServiceError>;

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
/// JSON, see <https://stripe.com/docs/api/customers/create>,
/// <https://stripe.com/docs/api/checkout/sessions/create>, and
/// <https://stripe.com/docs/api/customer_portal/sessions/create>.
pub struct StripeServiceImpl {
    client: reqwest::Client,
    api_key: String,
    api_base_url: String,
    /// URL the user is redirected to after a successful checkout.
    checkout_success_url: String,
    /// URL the user is redirected to when they cancel checkout.
    checkout_cancel_url: String,
    /// URL the user is redirected to after closing the customer-portal.
    portal_return_url: String,
}

impl StripeServiceImpl {
    pub fn new(
        api_key: String,
        checkout_success_url: String,
        checkout_cancel_url: String,
        portal_return_url: String,
    ) -> Self {
        Self {
            client: reqwest::Client::new(),
            api_key,
            api_base_url: "https://api.stripe.com".to_owned(),
            checkout_success_url,
            checkout_cancel_url,
            portal_return_url,
        }
    }

    async fn post_form(
        &self,
        path: &str,
        form: &[(&str, String)],
    ) -> Result<serde_json::Value, StripeServiceError> {
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

        Ok(response.json().await?)
    }
}

#[async_trait]
impl StripeService for StripeServiceImpl {
    async fn create_customer(
        &self,
        customer: &CreateStripeCustomerCommand,
    ) -> Result<StripeCustomerId, StripeServiceError> {
        let mut form: Vec<(&str, String)> = vec![
            ("email", customer.email.to_string()),
            ("metadata[userId]", customer.user_id.to_string()),
        ];
        if let Some(name) = customer.name.as_ref() {
            form.push(("name", name.to_string()));
        }

        let body = self.post_form("/v1/customers", &form).await?;
        body.get("id")
            .and_then(|v| v.as_str())
            .map(StripeCustomerId::from)
            .ok_or(StripeServiceError::MissingField("id"))
    }

    async fn create_checkout_session(
        &self,
        user_id: &UserId,
        stripe_customer_id: &StripeCustomerId,
        price_id: &str,
    ) -> Result<Url, StripeServiceError> {
        let form = vec![
            ("mode", "subscription".to_owned()),
            ("customer", stripe_customer_id.to_string()),
            ("line_items[0][price]", price_id.to_owned()),
            ("line_items[0][quantity]", "1".to_owned()),
            ("billing_address_collection", "required".to_owned()),
            ("automatic_tax[enabled]", "true".to_owned()),
            // Optional tax-id collection: customers can supply a VAT/tax id
            // but are not required to.
            ("tax_id_collection[enabled]", "true".to_owned()),
            // Optional business-name collection via a custom-field. Stripe
            // does not expose a dedicated `business_name_collection` flag,
            // so we use the documented `custom_fields` mechanism with
            // `optional=true` to allow (but not require) entering it.
            ("custom_fields[0][key]", "business_name".to_owned()),
            ("custom_fields[0][type]", "text".to_owned()),
            ("custom_fields[0][label][type]", "custom".to_owned()),
            (
                "custom_fields[0][label][custom]",
                "Business name".to_owned(),
            ),
            ("custom_fields[0][optional]", "true".to_owned()),
            ("client_reference_id", user_id.to_string()),
            ("metadata[userId]", user_id.to_string()),
            ("subscription_data[metadata][userId]", user_id.to_string()),
            ("success_url", self.checkout_success_url.clone()),
            ("cancel_url", self.checkout_cancel_url.clone()),
        ];

        let body = self.post_form("/v1/checkout/sessions", &form).await?;
        body.get("url")
            .and_then(|v| v.as_str())
            .and_then(|s| Url::parse(s).ok())
            .ok_or(StripeServiceError::MissingField("url"))
    }

    async fn create_portal_session(
        &self,
        user_id: &UserId,
        stripe_customer_id: &StripeCustomerId,
    ) -> Result<Url, StripeServiceError> {
        let form = vec![
            ("customer", stripe_customer_id.to_string()),
            ("metadata[userId]", user_id.to_string()),
            ("return_url", self.portal_return_url.clone()),
        ];

        let body = self.post_form("/v1/billing_portal/sessions", &form).await?;
        body.get("url")
            .and_then(|v| v.as_str())
            .and_then(|s| Url::parse(s).ok())
            .ok_or(StripeServiceError::MissingField("url"))
    }
}
