use crate::core::{first_name::FirstName, last_name::LastName};
use common::{currency::domain::Currency, language::domain::Language, user_id::UserId};
use serde_email::Email;

#[derive(Debug, Clone, PartialEq)]
pub struct CreateUserCommand {
    pub id: UserId,
    pub email: Email,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct UpdateUserCommand {
    pub first_name: Option<FirstName>,
    pub last_name: Option<LastName>,
    pub language: Option<Language>,
    pub currency: Option<Currency>,
    pub prohibited_content_consent: Option<bool>,
}

impl UpdateUserCommand {
    pub fn is_empty(&self) -> bool {
        self.first_name.is_none()
            && self.last_name.is_none()
            && self.language.is_none()
            && self.currency.is_none()
            && self.prohibited_content_consent.is_none()
    }
}

#[cfg(feature = "test-data")]
mod fake {
    use crate::service::command::{CreateUserCommand, UpdateUserCommand};
    use fake::{Fake, faker::internet::de_de::SafeEmail};

    impl fake::Dummy<fake::Faker> for CreateUserCommand {
        fn dummy_with_rng<R: fake::rand::Rng + ?Sized>(config: &fake::Faker, rng: &mut R) -> Self {
            let email_str: String = SafeEmail().fake_with_rng(rng);
            CreateUserCommand {
                id: config.fake_with_rng(rng),
                email: email_str.try_into().unwrap(),
            }
        }
    }

    impl fake::Dummy<fake::Faker> for UpdateUserCommand {
        fn dummy_with_rng<R: fake::rand::Rng + ?Sized>(config: &fake::Faker, rng: &mut R) -> Self {
            UpdateUserCommand {
                first_name: config.fake_with_rng(rng),
                last_name: config.fake_with_rng(rng),
                language: config.fake_with_rng(rng),
                currency: config.fake_with_rng(rng),
                prohibited_content_consent: config.fake_with_rng(rng),
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
