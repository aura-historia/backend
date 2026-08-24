use credential_core::{oauth_client_id::OAuthClientId, scope::Scope};
use domain_primitives::{change_outcome::ChangeOutcome, string_newtype};
use std::collections::HashSet;
use url::Url;
use user_core::access_token::HashedRawOAuthClientSecret;

string_newtype!(
    OAuthClientName,
    derives(serde::Serialize, serde::Deserialize)
);

#[derive(Debug, Clone, PartialEq)]
pub struct OAuthClient {
    client_id: OAuthClientId,
    hashed_client_secret: HashedRawOAuthClientSecret,
    name: OAuthClientName,
    redirect_uris: HashSet<Url>,
    tos_uri: Url,
    policy_uri: Url,
    client_uri: Url,
    logo_uri: Url,
    scopes: HashSet<Scope>,
}

#[doc(hidden)]
#[derive(Debug, Clone, PartialEq)]
pub struct RehydratedOAuthClientState {
    pub client_id: OAuthClientId,
    pub hashed_client_secret: HashedRawOAuthClientSecret,
    pub name: OAuthClientName,
    pub redirect_uris: HashSet<Url>,
    pub tos_uri: Url,
    pub policy_uri: Url,
    pub client_uri: Url,
    pub logo_uri: Url,
    pub scopes: HashSet<Scope>,
}

impl OAuthClient {
    pub fn create(state: RehydratedOAuthClientState) -> Self {
        Self::rehydrate(state)
    }

    #[doc(hidden)]
    pub fn rehydrate(state: RehydratedOAuthClientState) -> Self {
        Self {
            client_id: state.client_id,
            hashed_client_secret: state.hashed_client_secret,
            name: state.name,
            redirect_uris: state.redirect_uris,
            tos_uri: state.tos_uri,
            policy_uri: state.policy_uri,
            client_uri: state.client_uri,
            logo_uri: state.logo_uri,
            scopes: state.scopes,
        }
    }

    pub fn client_id(&self) -> OAuthClientId {
        self.client_id
    }

    pub fn hashed_client_secret(&self) -> &HashedRawOAuthClientSecret {
        &self.hashed_client_secret
    }

    pub fn name(&self) -> &OAuthClientName {
        &self.name
    }

    pub fn redirect_uris(&self) -> &HashSet<Url> {
        &self.redirect_uris
    }

    pub fn tos_uri(&self) -> &Url {
        &self.tos_uri
    }

    pub fn policy_uri(&self) -> &Url {
        &self.policy_uri
    }

    pub fn client_uri(&self) -> &Url {
        &self.client_uri
    }

    pub fn logo_uri(&self) -> &Url {
        &self.logo_uri
    }

    pub fn scopes(&self) -> &HashSet<Scope> {
        &self.scopes
    }

    pub fn change_name(&mut self, name: OAuthClientName) -> ChangeOutcome {
        replace_if_changed(&mut self.name, name)
    }

    pub fn replace_redirect_uris(&mut self, redirect_uris: HashSet<Url>) -> ChangeOutcome {
        replace_if_changed(&mut self.redirect_uris, redirect_uris)
    }

    pub fn change_tos_uri(&mut self, tos_uri: Url) -> ChangeOutcome {
        replace_if_changed(&mut self.tos_uri, tos_uri)
    }

    pub fn change_policy_uri(&mut self, policy_uri: Url) -> ChangeOutcome {
        replace_if_changed(&mut self.policy_uri, policy_uri)
    }

    pub fn change_client_uri(&mut self, client_uri: Url) -> ChangeOutcome {
        replace_if_changed(&mut self.client_uri, client_uri)
    }

    pub fn change_logo_uri(&mut self, logo_uri: Url) -> ChangeOutcome {
        replace_if_changed(&mut self.logo_uri, logo_uri)
    }

    pub fn replace_scopes(&mut self, scopes: HashSet<Scope>) -> ChangeOutcome {
        replace_if_changed(&mut self.scopes, scopes)
    }
}

fn replace_if_changed<T: PartialEq>(target: &mut T, value: T) -> ChangeOutcome {
    if *target == value {
        ChangeOutcome::Unchanged
    } else {
        *target = value;
        ChangeOutcome::Changed
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use user_core::access_token::RawOAuthClientSecret;

    fn url(value: &str) -> Url {
        match Url::parse(value) {
            Ok(url) => url,
            Err(error) => panic!("test URL must be valid: {error}"),
        }
    }

    fn client() -> OAuthClient {
        let secret = RawOAuthClientSecret::new();
        OAuthClient::create(RehydratedOAuthClientState {
            client_id: OAuthClientId::new(),
            hashed_client_secret: HashedRawOAuthClientSecret::from(secret),
            name: OAuthClientName::from("Client"),
            redirect_uris: HashSet::from([url("https://client.example/callback")]),
            tos_uri: url("https://client.example/tos"),
            policy_uri: url("https://client.example/policy"),
            client_uri: url("https://client.example"),
            logo_uri: url("https://client.example/logo.png"),
            scopes: HashSet::from([Scope::ProductsWrite]),
        })
    }

    #[test]
    fn should_report_unchanged_for_equal_client_metadata() {
        let mut client = client();

        assert_eq!(
            ChangeOutcome::Unchanged,
            client.change_name(client.name().clone())
        );
        assert_eq!(
            ChangeOutcome::Unchanged,
            client.replace_redirect_uris(client.redirect_uris().clone())
        );
        assert_eq!(
            ChangeOutcome::Unchanged,
            client.replace_scopes(client.scopes().clone())
        );
    }

    #[test]
    fn should_replace_client_metadata_through_domain_methods() {
        let mut client = client();
        let redirect_uris = HashSet::from([url("https://client.example/new-callback")]);
        let scopes = HashSet::from([Scope::ShopsRead]);

        assert_eq!(
            ChangeOutcome::Changed,
            client.change_name(OAuthClientName::from("Renamed"))
        );
        assert_eq!(
            ChangeOutcome::Changed,
            client.replace_redirect_uris(redirect_uris.clone())
        );
        assert_eq!(
            ChangeOutcome::Changed,
            client.change_tos_uri(url("https://client.example/new-tos"))
        );
        assert_eq!(
            ChangeOutcome::Changed,
            client.change_policy_uri(url("https://client.example/new-policy"))
        );
        assert_eq!(
            ChangeOutcome::Changed,
            client.change_client_uri(url("https://new-client.example"))
        );
        assert_eq!(
            ChangeOutcome::Changed,
            client.change_logo_uri(url("https://client.example/new-logo.png"))
        );
        assert_eq!(
            ChangeOutcome::Changed,
            client.replace_scopes(scopes.clone())
        );

        assert_eq!(&OAuthClientName::from("Renamed"), client.name());
        assert_eq!(&redirect_uris, client.redirect_uris());
        assert_eq!(&scopes, client.scopes());
    }
}
