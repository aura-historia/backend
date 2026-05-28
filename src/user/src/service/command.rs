use crate::core::{
    access_token::{AccessTokenName, AccessTokenOrigin, Scope},
    first_name::FirstName,
    last_name::LastName,
    role::UserRole,
    tier::UserTier,
};
use common::{
    currency::domain::Currency, language::domain::Language, stripe_customer_id::StripeCustomerId,
    user_id::UserId,
};
use geo::core::address::StructuredAddress;
use serde_email::Email;
use std::collections::HashSet;
use time::OffsetDateTime;

#[derive(Debug, Clone, PartialEq)]
pub struct CreateUserCommand {
    pub id: UserId,
    pub email: Email,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CreateAccessTokenCommand {
    pub name: AccessTokenName,
    pub scopes: HashSet<Scope>,
    pub expires: Option<OffsetDateTime>,
    pub origin: AccessTokenOrigin,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct UpdateAccessTokenCommand {
    pub name: Option<AccessTokenName>,
    pub scopes: Option<HashSet<Scope>>,
    pub expires: Option<OffsetDateTime>,
}

impl UpdateAccessTokenCommand {
    pub fn is_empty(&self) -> bool {
        self.name.is_none() && self.scopes.is_none() && self.expires.is_none()
    }
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct UpdateUserCommand {
    pub first_name: Option<FirstName>,
    pub last_name: Option<LastName>,
    pub language: Option<Language>,
    pub currency: Option<Currency>,
    pub prohibited_content_consent: Option<bool>,
    pub tier: Option<UserTier>,
    pub role: Option<UserRole>,
    pub stripe_customer_id: Option<StripeCustomerId>,
    pub structured_address: Option<StructuredAddress>,
}

impl UpdateUserCommand {
    pub fn is_empty(&self) -> bool {
        self.first_name.is_none()
            && self.last_name.is_none()
            && self.language.is_none()
            && self.currency.is_none()
            && self.prohibited_content_consent.is_none()
            && self.tier.is_none()
            && self.role.is_none()
            && self.stripe_customer_id.is_none()
            && self.structured_address.is_none()
    }
}

#[cfg(feature = "test-data")]
mod fake {
    use crate::service::command::{CreateUserCommand, UpdateUserCommand};
    use fake::{Fake, faker::internet::de_de::SafeEmail};

    impl fake::Dummy<fake::Faker> for CreateUserCommand {
        fn dummy_with_rng<R: fake::rand::RngExt + ?Sized>(
            config: &fake::Faker,
            rng: &mut R,
        ) -> Self {
            let email_str: String = SafeEmail().fake_with_rng(rng);
            CreateUserCommand {
                id: config.fake_with_rng(rng),
                email: email_str.try_into().unwrap(),
            }
        }
    }

    impl fake::Dummy<fake::Faker> for UpdateUserCommand {
        fn dummy_with_rng<R: fake::rand::RngExt + ?Sized>(
            config: &fake::Faker,
            rng: &mut R,
        ) -> Self {
            UpdateUserCommand {
                first_name: config.fake_with_rng(rng),
                last_name: config.fake_with_rng(rng),
                language: config.fake_with_rng(rng),
                currency: config.fake_with_rng(rng),
                prohibited_content_consent: config.fake_with_rng(rng),
                tier: config.fake_with_rng(rng),
                role: config.fake_with_rng(rng),
                stripe_customer_id: config.fake_with_rng(rng),
                structured_address: None,
            }
        }
    }

    #[cfg(test)]
    mod tests {
        use crate::service::command::CreateUserCommand;
        use fake::{Fake, Faker};

        #[test]
        fn should_fake_create_user_command() {
            let _ = Faker.fake::<CreateUserCommand>();
        }
    }
}
