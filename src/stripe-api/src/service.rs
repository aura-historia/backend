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
use common::{currency::domain::Currency, stripe_customer_id::StripeCustomerId, user_id::UserId};
use serde_email::Email;
use std::collections::HashMap;
use std::collections::HashSet;
use thiserror::Error;
use tokio::sync::OnceCell;
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

/// Information about a Stripe `Price` resolved by its lookup key.
///
/// Carries the `price_id` and the set of currency codes (lowercase ISO 4217)
/// that this price supports — both the price's native currency and any
/// explicitly configured `currency_options`.
#[derive(Debug, Clone, PartialEq)]
pub struct StripePriceInfo {
    /// The Stripe `price_…` id.
    pub id: String,
    /// All currency codes (lowercase) this price can be presented in, including
    /// its native currency and all `currency_options` keys.
    pub supported_currencies: HashSet<String>,
}

impl StripePriceInfo {
    /// Returns the lowercase currency code to forward to a Stripe checkout
    /// session for the given user preference, or `None` when the currency is
    /// not explicitly supported (falling back to Stripe adaptive pricing).
    pub fn select_currency(&self, preferred: Option<&Currency>) -> Option<String> {
        let code = preferred?.as_str().to_lowercase();
        self.supported_currencies.contains(&code).then_some(code)
    }
}

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

    /// Resolve a Stripe `Price` by its lookup key and return basic price
    /// information including all supported currencies.
    ///
    /// Implementations are expected to cache the result in memory so that
    /// the Stripe API is only called once per Lambda lifecycle (warm
    /// invocations re-use the cached value).
    async fn get_price_by_lookup_key(
        &self,
        lookup_key: &str,
    ) -> Result<StripePriceInfo, StripeServiceError>;

    /// Create a new Stripe Checkout-Session for an *existing* Stripe customer
    /// and return the hosted Checkout URL.
    ///
    /// When `currency` is `Some`, the session is created with that explicit
    /// currency (maps to a `currency_options` entry on the price).  When
    /// `None`, no currency is forwarded and Stripe's adaptive-pricing engine
    /// determines the best match for the customer's location.
    async fn create_checkout_session(
        &self,
        user_id: &UserId,
        stripe_customer_id: &StripeCustomerId,
        price_id: &str,
        currency: Option<&str>,
    ) -> Result<Url, StripeServiceError>;

    /// Create a new Stripe Billing-Portal-Session for the given customer and
    /// return the hosted Portal URL.
    async fn create_portal_session(
        &self,
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
    /// In-memory cache for prices looked up by lookup key.  Populated on the
    /// first invocation and reused on subsequent warm Lambda invocations.
    price_cache: OnceCell<HashMap<String, StripePriceInfo>>,
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
            price_cache: OnceCell::new(),
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

    async fn get_query(
        &self,
        path: &str,
        params: &[(&str, &str)],
    ) -> Result<serde_json::Value, StripeServiceError> {
        let url = format!("{}{}", self.api_base_url, path);
        let response = self
            .client
            .get(url)
            .bearer_auth(&self.api_key)
            .query(params)
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

    /// Fetches all known prices from Stripe in a single API call and returns
    /// the populated lookup-key → [`StripePriceInfo`] map.
    async fn fetch_all_prices(
        &self,
    ) -> Result<HashMap<String, StripePriceInfo>, StripeServiceError> {
        const LOOKUP_KEYS: [&str; 4] = [
            "pro_monthly",
            "pro_yearly",
            "ultimate_monthly",
            "ultimate_yearly",
        ];

        let mut params: Vec<(&str, &str)> =
            LOOKUP_KEYS.iter().map(|k| ("lookup_keys[]", *k)).collect();
        params.push(("expand[]", "data.currency_options"));

        let body = self.get_query("/v1/prices", &params).await?;

        let data = body
            .get("data")
            .and_then(|v| v.as_array())
            .ok_or(StripeServiceError::MissingField("data"))?;

        let mut map = HashMap::new();
        for price in data {
            let id = price
                .get("id")
                .and_then(|v| v.as_str())
                .ok_or(StripeServiceError::MissingField("id"))?
                .to_owned();

            let lookup_key = price
                .get("lookup_key")
                .and_then(|v| v.as_str())
                .ok_or(StripeServiceError::MissingField("lookup_key"))?
                .to_owned();

            let native_currency = price
                .get("currency")
                .and_then(|v| v.as_str())
                .ok_or(StripeServiceError::MissingField("currency"))?
                .to_owned();

            let mut supported_currencies: HashSet<String> = HashSet::new();
            supported_currencies.insert(native_currency);

            if let Some(currency_options) =
                price.get("currency_options").and_then(|v| v.as_object())
            {
                for key in currency_options.keys() {
                    supported_currencies.insert(key.clone());
                }
            }

            map.insert(
                lookup_key,
                StripePriceInfo {
                    id,
                    supported_currencies,
                },
            );
        }

        Ok(map)
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

    async fn get_price_by_lookup_key(
        &self,
        lookup_key: &str,
    ) -> Result<StripePriceInfo, StripeServiceError> {
        let cache = self
            .price_cache
            .get_or_try_init(|| self.fetch_all_prices())
            .await?;

        cache
            .get(lookup_key)
            .cloned()
            .ok_or_else(|| StripeServiceError::Api {
                status: 404,
                body: format!("No price found for lookup key '{lookup_key}'"),
            })
    }

    async fn create_checkout_session(
        &self,
        user_id: &UserId,
        stripe_customer_id: &StripeCustomerId,
        price_id: &str,
        currency: Option<&str>,
    ) -> Result<Url, StripeServiceError> {
        let mut form = vec![
            ("mode", "subscription".to_owned()),
            ("customer", stripe_customer_id.to_string()),
            ("customer_update[address]", "auto".to_owned()),
            ("customer_update[name]", "auto".to_owned()),
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
            ("subscription_data[metadata][userId]", user_id.to_string()),
            ("success_url", self.checkout_success_url.clone()),
            ("cancel_url", self.checkout_cancel_url.clone()),
        ];

        if let Some(c) = currency {
            form.push(("currency", c.to_owned()));
        }

        let body = self.post_form("/v1/checkout/sessions", &form).await?;
        body.get("url")
            .and_then(|v| v.as_str())
            .and_then(|s| Url::parse(s).ok())
            .ok_or(StripeServiceError::MissingField("url"))
    }

    async fn create_portal_session(
        &self,
        stripe_customer_id: &StripeCustomerId,
    ) -> Result<Url, StripeServiceError> {
        let form = vec![
            ("customer", stripe_customer_id.to_string()),
            ("return_url", self.portal_return_url.clone()),
        ];

        let body = self.post_form("/v1/billing_portal/sessions", &form).await?;
        body.get("url")
            .and_then(|v| v.as_str())
            .and_then(|s| Url::parse(s).ok())
            .ok_or(StripeServiceError::MissingField("url"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rstest::rstest;

    fn price_info_with_currencies(currencies: &[&str]) -> StripePriceInfo {
        StripePriceInfo {
            id: "price_test".to_owned(),
            supported_currencies: currencies.iter().map(|s| s.to_string()).collect(),
        }
    }

    #[rstest]
    #[case(Some(Currency::Eur), &["eur", "usd"], Some("eur"))]
    #[case(Some(Currency::Usd), &["eur", "usd"], Some("usd"))]
    #[case(Some(Currency::Gbp), &["eur", "usd"], None)]
    #[case(None, &["eur", "usd"], None)]
    fn should_select_currency_when_preference_given(
        #[case] preferred: Option<Currency>,
        #[case] supported: &[&str],
        #[case] expected: Option<&str>,
    ) {
        let info = price_info_with_currencies(supported);
        assert_eq!(
            info.select_currency(preferred.as_ref()),
            expected.map(str::to_owned)
        );
    }
}
