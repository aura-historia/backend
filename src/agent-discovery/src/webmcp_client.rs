//! The website's own OAuth Client ID Metadata Document. The website signs its users in to the
//! MCP server as an ordinary metadata document client, so agents in the browser can use the
//! signed-in MCP tools through WebMCP. Its `client_id` is the URL the website publishes it at,
//! and it asks for nothing that other metadata document clients cannot have.
use crate::{DiscoveryEnvironment, site_url};
use oauth_core::resource::AuraResourcePolicy;
use serde::Serialize;

pub(crate) const PATH: &str = "oauth/webmcp-client.json";
/// The website's Worker route that receives the authorization response and exchanges the code.
const CALLBACK_PATH: &str = "/api/oauth/webmcp/callback";
/// Shown on the consent page beside the verified host of the `client_id`.
const CLIENT_NAME: &str = "AI agents in your browser";

#[derive(Serialize)]
struct ClientMetadata {
    client_id: String,
    client_name: &'static str,
    client_uri: &'static str,
    logo_uri: String,
    redirect_uris: Vec<String>,
    grant_types: [&'static str; 2],
    response_types: [&'static str; 1],
    token_endpoint_auth_method: &'static str,
}

/// The website's WebMCP callback.
pub(crate) fn callback_url(environment: DiscoveryEnvironment) -> String {
    format!("{}{CALLBACK_PATH}", environment.site_origin())
}

/// The same callback on the webapp's local development server, which signs in against stage.
/// A loopback callback matches on any port.
pub(crate) fn development_callback_url() -> String {
    format!("http://localhost{CALLBACK_PATH}")
}

fn redirect_uris(environment: DiscoveryEnvironment) -> Vec<String> {
    let mut redirect_uris = vec![callback_url(environment)];
    if environment == AuraResourcePolicy::stage().into() {
        redirect_uris.push(development_callback_url());
    }
    redirect_uris
}

pub(crate) fn render(environment: DiscoveryEnvironment) -> String {
    let document = ClientMetadata {
        client_id: site_url(environment, PATH),
        client_name: CLIENT_NAME,
        client_uri: environment.site_origin(),
        logo_uri: environment.site_logo_url(),
        redirect_uris: redirect_uris(environment),
        grant_types: ["authorization_code", "refresh_token"],
        response_types: ["code"],
        token_endpoint_auth_method: "none",
    };
    serde_json::to_string_pretty(&document).expect("the client metadata document serializes")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::SiteDocuments;
    use serde_json::{Value, json};

    /// The API's agent-discovery tests load the document from the website with the OAuth
    /// endpoints' document source and validator.
    #[test]
    fn should_name_the_url_the_website_publishes_it_at_as_its_client_id() {
        for (policy, url) in [
            (
                AuraResourcePolicy::production(),
                "https://aura-historia.com/oauth/webmcp-client.json",
            ),
            (
                AuraResourcePolicy::stage(),
                "https://stage.aura-historia.com/oauth/webmcp-client.json",
            ),
        ] {
            let environment = DiscoveryEnvironment::from(policy);
            let documents = SiteDocuments::render(environment);
            let published = documents.get(PATH).unwrap();
            assert_eq!(
                url,
                format!("{}{}", environment.site_origin(), published.site_path())
            );
            let body: Value = serde_json::from_str(published.body()).unwrap();
            assert_eq!(url, body["client_id"]);
        }
    }

    #[test]
    fn should_name_only_website_urls_and_the_development_callback_on_stage_only() {
        let production: Value =
            serde_json::from_str(&render(AuraResourcePolicy::production().into())).unwrap();
        assert_eq!(
            json!({
                "client_id": "https://aura-historia.com/oauth/webmcp-client.json",
                "client_name": "AI agents in your browser",
                "client_uri": "https://aura-historia.com",
                "logo_uri": "https://aura-historia.com/icon1080x1080.png",
                "redirect_uris": ["https://aura-historia.com/api/oauth/webmcp/callback"],
                "grant_types": ["authorization_code", "refresh_token"],
                "response_types": ["code"],
                "token_endpoint_auth_method": "none",
            }),
            production
        );

        let stage: Value =
            serde_json::from_str(&render(AuraResourcePolicy::stage().into())).unwrap();
        assert_eq!(
            json!([
                "https://stage.aura-historia.com/api/oauth/webmcp/callback",
                "http://localhost/api/oauth/webmcp/callback",
            ]),
            stage["redirect_uris"]
        );
        for url in ["client_id", "client_uri", "logo_uri"] {
            assert!(
                stage[url]
                    .as_str()
                    .unwrap()
                    .starts_with("https://stage.aura-historia.com")
            );
        }
    }
}
