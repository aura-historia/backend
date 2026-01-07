use crate::{mail_id::MailId, payload::MailPayload, template::MailTemplate};
use common::user_id::UserId;
use serde::{Deserialize, Serialize};
use serde_email::Email;
use time::OffsetDateTime;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MailRecord {
    pub pk: String,
    pub sk: String,
    pub user_id: UserId,
    pub mail_id: MailId,
    pub sender: Email,
    pub recipient: Email,
    pub subject: String,
    pub template: MailTemplate,
    pub data: serde_json::Value,

    #[serde(with = "time::serde::rfc3339")]
    pub created: OffsetDateTime,
}

pub fn mk_pk(user_id: &UserId) -> String {
    format!("user#{user_id}")
}

pub fn mk_sk(mail_id: &MailId) -> String {
    format!("mail#{mail_id}")
}

impl From<MailPayload> for MailRecord {
    fn from(payload: MailPayload) -> Self {
        MailRecord {
            pk: mk_pk(&payload.user_id),
            sk: mk_sk(&payload.mail_id),
            user_id: payload.user_id,
            mail_id: payload.mail_id,
            sender: payload.sender,
            recipient: payload.recipient,
            subject: payload.subject,
            template: payload.template,
            data: payload.data,
            created: OffsetDateTime::now_utc(),
        }
    }
}

#[cfg(feature = "test-data")]
mod faker {
    use super::*;
    use fake::{Dummy, Fake, Faker, Rng, faker::internet::de_de::SafeEmail};
    use serde_json::json;

    impl Dummy<Faker> for MailRecord {
        fn dummy_with_rng<R: Rng + ?Sized>(config: &Faker, rng: &mut R) -> Self {
            let user_id = UserId::new();
            let mail_id = MailId::new();
            MailRecord {
                pk: mk_pk(&user_id),
                sk: mk_sk(&mail_id),
                user_id,
                mail_id,
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
                data: json!({
                    "foo": config.fake_with_rng::<String, _>(rng),
                    "bar": config.fake_with_rng::<String, _>(rng),
                    "baz": config.fake_with_rng::<bool, _>(rng)
                }),
                created: OffsetDateTime::now_utc(),
            }
        }
    }

    #[cfg(test)]
    mod tests {
        use crate::record::MailRecord;
        use fake::{Fake, Faker};

        #[test]
        fn should_fake_mail_record() {
            let _ = Faker.fake::<MailRecord>();
        }
    }
}
