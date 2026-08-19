use domain_primitives::string_newtype;
use serde::{Deserialize, Serialize};

string_newtype!(WoocommerceWebhookSecret, derives(Serialize, Deserialize));

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_create_webhook_secret_from_str() {
        let secret = WoocommerceWebhookSecret::from("secret-value");

        assert_eq!("secret-value", secret.as_ref());
    }

    #[test]
    fn should_create_webhook_secret_from_string() {
        let value = String::from("secret-value");
        let secret = WoocommerceWebhookSecret::from(value.clone());

        assert_eq!(value, secret.to_string());
    }
}
