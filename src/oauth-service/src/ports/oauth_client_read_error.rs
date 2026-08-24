use application::error::BoxError;

#[derive(Debug, thiserror::Error)]
pub enum OAuthClientReadError {
    #[error("temporary OAuth client read failure")]
    TemporarilyUnavailable {
        #[source]
        source: BoxError,
    },
    #[error("invalid persisted OAuth client view")]
    InvalidPersistedState {
        #[source]
        source: BoxError,
    },
    #[error("internal OAuth client read failure")]
    Internal {
        #[source]
        source: BoxError,
    },
}
