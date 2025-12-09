use crate::core::{first_name::FirstName, last_name::LastName};
use common::{currency::domain::Currency, language::domain::Language, user_id::UserId};
use serde_email::Email;

#[derive(Debug, Clone, PartialEq)]
pub struct CreateUserCommand {
    pub id: UserId,
    pub email: Email,
}

#[derive(Debug, Clone, PartialEq)]
pub struct UpdateUserCommand {
    pub email: Option<Email>,
    pub first_name: Option<FirstName>,
    pub last_name: Option<LastName>,
    pub language: Option<Language>,
    pub currency: Option<Currency>,
}

#[cfg(feature = "test-data")]
mod fake {
    use crate::service::command::CreateUserCommand;
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
