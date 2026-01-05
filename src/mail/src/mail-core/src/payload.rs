use crate::template::MailTemplate;
use serde::{Deserialize, Serialize};
use serde_email::Email;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MailPayload {
    pub sender: Email,
    pub recipient: Email,
    pub subject: String,
    pub template: MailTemplate,
    pub data: serde_json::Value,
}

#[cfg(feature = "test-data")]
mod faker {
    use super::*;
    use fake::{Dummy, Fake, Faker, Rng, faker::internet::de_de::SafeEmail};

    impl Dummy<Faker> for MailPayload {
        fn dummy_with_rng<R: Rng + ?Sized>(config: &Faker, rng: &mut R) -> Self {
            MailPayload {
                sender: SafeEmail()
                    .fake_with_rng::<String, R>(rng)
                    .try_into()
                    .unwrap(),
                recipient: SafeEmail()
                    .fake_with_rng::<String, R>(rng)
                    .try_into()
                    .unwrap(),
                subject: config.fake_with_rng(rng),
                template: config.fake_with_rng(rng),
                data: config.fake_with_rng(rng),
            }
        }
    }

    #[cfg(test)]
    mod tests {
        use crate::payload::MailPayload;
        use fake::{Fake, Faker};

        #[test]
        fn should_fake_mail_payload() {
            let _ = Faker.fake::<MailPayload>();
        }
    }
}
