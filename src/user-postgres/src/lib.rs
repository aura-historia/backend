mod access_token_mapping;
mod cognito_identity;
mod consent_workflow_cleanup;
mod loops_webhook_receipts;
mod mapping;
mod marketing_consent_intents;
mod newsletter_confirmation_challenges;
mod readers;
mod repositories;

pub use cognito_identity::{SqlxCognitoUserIdentityReader, SqlxUserCognitoIdentityRegistryFactory};
pub use consent_workflow_cleanup::SqlxConsentWorkflowCleanup;
pub use loops_webhook_receipts::SqlxLoopsWebhookReceiptRepository;
pub use marketing_consent_intents::{
    ConsentIntentFinalization, ConsentIntentSource, ConsentIntentStatus, ConsentSubject,
    MarketingConsentIntent, MarketingConsentIntentClaim, MarketingConsentPersistenceError,
    SqlxMarketingConsentIntentRepository, SqlxMarketingConsentIntentWorker,
};
pub use newsletter_confirmation_challenges::SqlxNewsletterConfirmationChallengesRepository;
pub use readers::{
    SqlxAccessTokenAuthenticationReader, SqlxAccessTokenDetailsReader, SqlxAccessTokenListReader,
    SqlxAdminAccessTokenListReaderFactory, SqlxNewsletterProfileReader,
    SqlxUserAccountReaderFactory, SqlxUserAdminReaderFactory, SqlxUserAuthenticationReader,
    SqlxUserSearchReaderFactory, SqlxUserStripeCustomerReaderFactory,
    SqlxUserTierEntitlementsFactory,
};
pub use repositories::{SqlxAccessTokenRepositoryFactory, SqlxUserRepositoryFactory};
