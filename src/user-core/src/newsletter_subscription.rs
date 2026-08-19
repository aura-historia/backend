use crate::{first_name::FirstName, last_name::LastName};
use common::user_id::UserId;
use localization::Language;
use money::Currency;
use serde_email::Email;

#[derive(Debug, Clone, PartialEq)]
pub struct NewsletterSubscription {
    email: Email,
    first_name: Option<FirstName>,
    last_name: Option<LastName>,
    language: Option<Language>,
    currency: Option<Currency>,
    user_id: Option<UserId>,
}

impl NewsletterSubscription {
    pub fn new(
        email: Email,
        first_name: Option<FirstName>,
        last_name: Option<LastName>,
        language: Option<Language>,
        currency: Option<Currency>,
        user_id: Option<UserId>,
    ) -> Self {
        Self {
            email,
            first_name,
            last_name,
            language,
            currency,
            user_id,
        }
    }

    pub fn email(&self) -> &Email {
        &self.email
    }

    pub fn first_name(&self) -> Option<&FirstName> {
        self.first_name.as_ref()
    }

    pub fn last_name(&self) -> Option<&LastName> {
        self.last_name.as_ref()
    }

    pub fn language(&self) -> Option<Language> {
        self.language
    }

    pub fn currency(&self) -> Option<Currency> {
        self.currency
    }

    pub fn user_id(&self) -> Option<UserId> {
        self.user_id
    }
}

#[cfg(test)]
mod tests {
    use super::NewsletterSubscription;
    use crate::{first_name::FirstName, last_name::LastName};
    use common::user_id::UserId;
    use localization::Language;
    use money::Currency;

    #[test]
    fn should_preserve_newsletter_subscription_values() {
        let user_id = UserId::new();
        let subscription = NewsletterSubscription::new(
            "ada@example.com"
                .try_into()
                .unwrap_or_else(|error| panic!("invalid test email: {error}")),
            Some(FirstName::from("Ada")),
            Some(LastName::from("Lovelace")),
            Some(Language::En),
            Some(Currency::Eur),
            Some(user_id),
        );

        assert_eq!("ada@example.com", subscription.email().to_string());
        assert_eq!(Some("Ada"), subscription.first_name().map(AsRef::as_ref));
        assert_eq!(
            Some("Lovelace"),
            subscription.last_name().map(AsRef::as_ref)
        );
        assert_eq!(Some(Language::En), subscription.language());
        assert_eq!(Some(Currency::Eur), subscription.currency());
        assert_eq!(Some(user_id), subscription.user_id());
    }

    #[test]
    fn should_allow_anonymous_subscription_without_optional_values() {
        let subscription = NewsletterSubscription::new(
            "collector@example.com"
                .try_into()
                .unwrap_or_else(|error| panic!("invalid test email: {error}")),
            None,
            None,
            None,
            None,
            None,
        );

        assert_eq!("collector@example.com", subscription.email().to_string());
        assert_eq!(None, subscription.first_name());
        assert_eq!(None, subscription.last_name());
        assert_eq!(None, subscription.language());
        assert_eq!(None, subscription.currency());
        assert_eq!(None, subscription.user_id());
    }
}
