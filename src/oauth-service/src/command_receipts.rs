use crate::ports::OAuthClientView;
use application::{
    error::{BoxError, static_error},
    idempotency::{self as retry, CommandOutcome},
};
use serde_json::{Value, json};
impl CommandOutcome for OAuthClientView {
    fn encode(&self) -> Result<Value, BoxError> {
        Ok(
            json!({"client_id":self.client_id,"name":self.name,"redirect_uris":self.redirect_uris,"tos_uri":self.tos_uri,"policy_uri":self.policy_uri,"client_uri":self.client_uri,"logo_uri":self.logo_uri,"scopes":self.scopes.iter().map(|v|v.as_str()).collect::<Vec<_>>(),"created":retry::time_value(self.created)?,"updated":retry::time_value(self.updated)?}),
        )
    }
    fn decode(v: Value) -> Result<Self, BoxError> {
        Ok(Self {
            client_id: retry::field(&v, "client_id")?,
            name: retry::field(&v, "name")?,
            redirect_uris: oauth_core::client::OAuthRedirectUris::try_from(retry::field::<
                std::collections::HashSet<url::Url>,
            >(
                &v, "redirect_uris"
            )?)?
            .into_set(),
            tos_uri: retry::field(&v, "tos_uri")?,
            policy_uri: retry::field(&v, "policy_uri")?,
            client_uri: retry::field(&v, "client_uri")?,
            logo_uri: retry::field(&v, "logo_uri")?,
            scopes: retry::field::<Vec<String>>(&v, "scopes")?
                .into_iter()
                .map(|v| {
                    user_core::access_token::Scope::from_code(&v)
                        .ok_or_else(|| static_error("invalid OAuth scope"))
                })
                .collect::<Result<_, _>>()?,
            created: retry::timestamp(&v, "created")?,
            updated: retry::timestamp(&v, "updated")?,
        })
    }
}
pub(crate) fn sorted<T: ToString>(values: impl Iterator<Item = T>) -> Vec<String> {
    let mut values = values.map(|v| v.to_string()).collect::<Vec<_>>();
    values.sort_unstable();
    values
}

pub(crate) struct IssuedOAuthClient {
    pub client_id: credential_core::oauth_client_id::OAuthClientId,
}
application::command_outcome!(IssuedOAuthClient { client_id });
