use common::error::boxed::BoxError;
use common::{currency::domain::Currency, language::domain::Language, user_id::UserId};
use user_core::{first_name::FirstName, last_name::LastName};

#[derive(Debug, Clone, PartialEq, Default)]
pub struct NewsletterProfile {
    pub first_name: Option<FirstName>,
    pub last_name: Option<LastName>,
    pub language: Option<Language>,
    pub currency: Option<Currency>,
}

#[derive(Debug, thiserror::Error)]
pub enum NewsletterProfileReadError {
    #[error("temporary newsletter profile read failure")]
    TemporarilyUnavailable {
        #[source]
        source: BoxError,
    },
    #[error("invalid newsletter profile read model")]
    InvalidReadModel {
        #[source]
        source: BoxError,
    },
    #[error("internal newsletter profile read failure")]
    Internal {
        #[source]
        source: BoxError,
    },
}

#[async_trait::async_trait]
pub trait NewsletterProfileReader: Send + Sync {
    async fn find_by_user_id(
        &self,
        user_id: UserId,
    ) -> Result<Option<NewsletterProfile>, NewsletterProfileReadError>;
}
