use application::error::box_error;
use serde_email::Email;
use sqlx::{FromRow, PgPool};
use user_service::ports::{
    CognitoIdentity, CognitoIssuer, CognitoSubject, FederatedAccount, FederatedAccountReadError,
    FederatedAccountReader,
};

#[derive(Debug, Clone)]
pub struct SqlxFederatedAccountReader {
    pool: PgPool,
}

#[derive(Debug, FromRow)]
struct FederatedAccountRow {
    email: String,
    issuer: Option<String>,
    subject: Option<String>,
}

impl SqlxFederatedAccountReader {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait::async_trait]
impl FederatedAccountReader for SqlxFederatedAccountReader {
    async fn find_by_email(
        &self,
        email: &Email,
    ) -> Result<Option<FederatedAccount>, FederatedAccountReadError> {
        let row = sqlx::query_as::<_, FederatedAccountRow>(
            "SELECT u.email, i.issuer, i.subject FROM users u \
             LEFT JOIN user_cognito_identities i ON i.user_id = u.user_id \
             WHERE u.email = $1",
        )
        .bind::<&str>(email.as_ref())
        .fetch_optional(&self.pool)
        .await
        .map_err(|source| FederatedAccountReadError::TemporarilyUnavailable {
            source: box_error(source),
        })?;

        row.map(FederatedAccount::try_from).transpose()
    }
}

impl TryFrom<FederatedAccountRow> for FederatedAccount {
    type Error = FederatedAccountReadError;

    fn try_from(row: FederatedAccountRow) -> Result<Self, Self::Error> {
        let email = row.email.as_str().try_into().map_err(|source| {
            FederatedAccountReadError::InvalidPersistedState {
                source: box_error(source),
            }
        })?;
        let cognito_identity = match (row.issuer, row.subject) {
            (None, None) => None,
            (Some(issuer), Some(subject)) => Some(CognitoIdentity {
                issuer: CognitoIssuer::try_from(issuer).map_err(|source| {
                    FederatedAccountReadError::InvalidPersistedState {
                        source: box_error(source),
                    }
                })?,
                subject: CognitoSubject::try_from(subject).map_err(|source| {
                    FederatedAccountReadError::InvalidPersistedState {
                        source: box_error(source),
                    }
                })?,
            }),
            _ => {
                return Err(FederatedAccountReadError::InvalidPersistedState {
                    source: box_error(std::io::Error::other(
                        "incomplete persisted Cognito identity binding",
                    )),
                });
            }
        };

        Ok(Self {
            email,
            cognito_identity,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_exact_email_and_optional_cognito_binding() {
        let account = FederatedAccount::try_from(FederatedAccountRow {
            email: "person@example.test".to_owned(),
            issuer: Some("https://cognito-idp.eu-central-1.amazonaws.com/pool".to_owned()),
            subject: Some("opaque|subject-1".to_owned()),
        })
        .unwrap_or_else(|error| panic!("valid federated account rejected: {error}"));

        assert_eq!(
            "person@example.test",
            <Email as AsRef<str>>::as_ref(&account.email)
        );
        let identity = account
            .cognito_identity
            .unwrap_or_else(|| panic!("expected Cognito binding"));
        assert_eq!("opaque|subject-1", identity.subject.as_str());
    }

    #[test]
    fn permits_a_user_without_a_cognito_binding_but_rejects_partial_binding() {
        let account = FederatedAccount::try_from(FederatedAccountRow {
            email: "person@example.test".to_owned(),
            issuer: None,
            subject: None,
        })
        .unwrap_or_else(|error| panic!("valid unbound account rejected: {error}"));
        assert!(account.cognito_identity.is_none());

        assert!(matches!(
            FederatedAccount::try_from(FederatedAccountRow {
                email: "person@example.test".to_owned(),
                issuer: Some("https://cognito-idp.eu-central-1.amazonaws.com/pool".to_owned()),
                subject: None,
            }),
            Err(FederatedAccountReadError::InvalidPersistedState { .. })
        ));
    }
}
