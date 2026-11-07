use crate::client::OAuthClientName;
use credential_core::metadata_document_client_id::MetadataDocumentClientId;
use credential_core::oauth_connection_id::OAuthConnectionId;
use credential_core::scope::Scope;
use std::collections::HashSet;
use time::OffsetDateTime;
use user_core::user_id::UserId;

/// One user's standing grant to one public client identified by its metadata document.
///
/// The connection groups everything the user granted that client: the union of the scopes
/// the user approved, the client's access tokens and its refresh-token families. Ending the
/// connection revokes all of them. See `docs/oauth.md`.
#[derive(Debug, Clone, PartialEq)]
pub struct OAuthConnection {
    id: OAuthConnectionId,
    user_id: UserId,
    client_id: MetadataDocumentClientId,
    client_name: OAuthClientName,
    scopes: HashSet<Scope>,
    created: OffsetDateTime,
    last_used: OffsetDateTime,
}

#[doc(hidden)]
#[derive(Debug, Clone, PartialEq)]
pub struct RehydratedOAuthConnectionState {
    pub id: OAuthConnectionId,
    pub user_id: UserId,
    pub client_id: MetadataDocumentClientId,
    pub client_name: OAuthClientName,
    pub scopes: HashSet<Scope>,
    pub created: OffsetDateTime,
    pub last_used: OffsetDateTime,
}

/// What the user approved for a client in one authorization request.
#[derive(Debug, Clone, PartialEq)]
pub struct OAuthConnectionApproval<'a> {
    pub user_id: UserId,
    pub client_id: &'a MetadataDocumentClientId,
    /// The name the client's validated document asserts at approval time.
    pub client_name: &'a OAuthClientName,
    pub scopes: &'a HashSet<Scope>,
    pub approved_at: OffsetDateTime,
}

impl OAuthConnection {
    /// The connection a user's first approval of a client starts.
    pub fn start(approval: OAuthConnectionApproval<'_>) -> Self {
        Self {
            id: OAuthConnectionId::new(),
            user_id: approval.user_id,
            client_id: approval.client_id.clone(),
            client_name: approval.client_name.clone(),
            scopes: approval.scopes.clone(),
            created: approval.approved_at,
            last_used: approval.approved_at,
        }
    }

    #[doc(hidden)]
    pub fn rehydrate(state: RehydratedOAuthConnectionState) -> Self {
        Self {
            id: state.id,
            user_id: state.user_id,
            client_id: state.client_id,
            client_name: state.client_name,
            scopes: state.scopes,
            created: state.created,
            last_used: state.last_used,
        }
    }

    /// A later approval, such as a step-up request, adds its scopes to the ones already
    /// granted and refreshes the name the client asserts.
    pub fn approve_again(&mut self, approval: OAuthConnectionApproval<'_>) {
        debug_assert_eq!(self.user_id, approval.user_id);
        debug_assert_eq!(&self.client_id, approval.client_id);
        self.client_name = approval.client_name.clone();
        self.scopes.extend(approval.scopes.iter().copied());
        self.record_use(approval.approved_at);
    }

    /// The client obtained credentials through this connection.
    pub fn record_use(&mut self, at: OffsetDateTime) {
        self.last_used = self.last_used.max(at);
    }

    pub fn id(&self) -> OAuthConnectionId {
        self.id
    }

    pub fn user_id(&self) -> UserId {
        self.user_id
    }

    pub fn client_id(&self) -> &MetadataDocumentClientId {
        &self.client_id
    }

    pub fn client_name(&self) -> &OAuthClientName {
        &self.client_name
    }

    /// The union of every scope the user approved for the client.
    pub fn scopes(&self) -> &HashSet<Scope> {
        &self.scopes
    }

    pub fn created(&self) -> OffsetDateTime {
        self.created
    }

    pub fn last_used(&self) -> OffsetDateTime {
        self.last_used
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use time::Duration;

    fn client_id() -> MetadataDocumentClientId {
        MetadataDocumentClientId::try_from("https://agent.example/client.json").unwrap()
    }

    #[test]
    fn should_add_step_up_scopes_to_the_scopes_already_granted() {
        let user_id = UserId::new();
        let client_id = client_id();
        let started = OffsetDateTime::now_utc();
        let mut connection = OAuthConnection::start(OAuthConnectionApproval {
            user_id,
            client_id: &client_id,
            client_name: &OAuthClientName::from("Agent"),
            scopes: &HashSet::from([Scope::UsersRead, Scope::WatchlistRead]),
            approved_at: started,
        });

        connection.approve_again(OAuthConnectionApproval {
            user_id,
            client_id: &client_id,
            client_name: &OAuthClientName::from("Agent v2"),
            scopes: &HashSet::from([Scope::UsersRead, Scope::SearchFiltersWrite]),
            approved_at: started + Duration::minutes(5),
        });

        assert_eq!(
            &HashSet::from([
                Scope::UsersRead,
                Scope::WatchlistRead,
                Scope::SearchFiltersWrite
            ]),
            connection.scopes()
        );
        assert_eq!("Agent v2", connection.client_name().as_ref());
        assert_eq!(started, connection.created());
        assert_eq!(started + Duration::minutes(5), connection.last_used());
    }

    #[test]
    fn should_never_move_last_use_backwards() {
        let client_id = client_id();
        let now = OffsetDateTime::now_utc();
        let mut connection = OAuthConnection::start(OAuthConnectionApproval {
            user_id: UserId::new(),
            client_id: &client_id,
            client_name: &OAuthClientName::from("Agent"),
            scopes: &HashSet::new(),
            approved_at: now,
        });

        connection.record_use(now - Duration::seconds(1));

        assert_eq!(now, connection.last_used());
    }
}
