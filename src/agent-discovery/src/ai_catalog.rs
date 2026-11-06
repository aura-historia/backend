//! The AI catalog ([Agentic Resource Discovery](https://agenticresourcediscovery.org/), in the
//! ai-catalog data model): one entry each for the MCP server, the REST API and the agent
//! skills index, identified by `urn:air:` names under the website's domain.
use crate::{DiscoveryEnvironment, SKILLS_INDEX_PATH, site_url};
use serde::Serialize;

pub(crate) const PATH: &str = "ai-catalog.json";
pub(crate) const MEDIA_TYPE: &str = "application/ai-catalog+json";
/// The ai-catalog data model version, not the ARD draft version.
const SPEC_VERSION: &str = "1.0";

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct AiCatalog {
    spec_version: &'static str,
    host: Host,
    entries: Vec<Entry>,
}

/// The website, identified by its domain because Aura Historia publishes no DID document.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Host {
    display_name: &'static str,
    identifier: &'static str,
    documentation_url: String,
    logo_url: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Entry {
    identifier: String,
    display_name: &'static str,
    #[serde(rename = "type")]
    media_type: &'static str,
    url: String,
    description: &'static str,
    representative_queries: &'static [&'static str],
}

pub(crate) fn render(environment: DiscoveryEnvironment) -> String {
    let domain = environment.site_domain();
    let urn = |namespace: &str, name: &str| format!("urn:air:{domain}:{namespace}:{name}");
    let catalog = AiCatalog {
        spec_version: SPEC_VERSION,
        host: Host {
            display_name: "Aura Historia",
            identifier: domain,
            documentation_url: format!("{}/", environment.documentation_origin()),
            logo_url: format!("{}/icon1080x1080.png", environment.site_origin()),
        },
        entries: vec![
            Entry {
                identifier: urn("mcp", "aura-historia"),
                display_name: "Aura Historia MCP server",
                media_type: "application/mcp-server-card+json",
                url: environment.mcp_server_card_url(),
                description: "Search art, antiques and design objects offered by dealers, auction houses, marketplaces and auction platforms without an account; sign in to manage a watchlist, saved searches and notifications.",
                representative_queries: &[
                    "find Art Nouveau silver vases offered by European dealers",
                    "which upcoming auctions include Meissen porcelain",
                    "show the price history of this antique clock listing",
                    "add this listing to my watchlist",
                ],
            },
            Entry {
                identifier: urn("api", "rest"),
                display_name: "Aura Historia REST API",
                media_type: "application/vnd.oai.openapi",
                url: format!("{}/swagger.yaml", environment.documentation_origin()),
                description: "OpenAPI description of the REST API for catalogue search, user accounts and partner listing synchronization.",
                representative_queries: &[
                    "search antique listings by price range and country",
                    "look up a listing source by its slug",
                    "synchronize a dealer's listings in batches",
                ],
            },
            Entry {
                identifier: urn("skills", "index"),
                display_name: "Aura Historia agent skills",
                media_type: "application/agent-skills+json",
                url: site_url(environment, SKILLS_INDEX_PATH),
                description: "Instructions for searching the catalogue, connecting a user's account and submitting partner listing batches.",
                representative_queries: &[
                    "how does an agent search the Aura Historia catalogue",
                    "sign a user in to Aura Historia from an agent",
                    "submit listing batches as an Aura Historia partner",
                ],
            },
        ],
    };
    serde_json::to_string_pretty(&catalog).expect("the AI catalog serializes")
}

#[cfg(test)]
mod tests {
    use super::*;
    use oauth_core::resource::AuraResourcePolicy;
    use serde_json::{Value, json};

    fn catalog(policy: AuraResourcePolicy) -> Value {
        serde_json::from_str(&render(policy.into())).unwrap()
    }

    #[test]
    fn should_catalog_the_mcp_server_rest_api_and_skills_for_the_website() {
        let catalog = catalog(AuraResourcePolicy::production());
        assert_eq!("1.0", catalog["specVersion"]);
        assert_eq!(
            json!({
                "displayName": "Aura Historia",
                "identifier": "aura-historia.com",
                "documentationUrl": "https://docs.api.aura-historia.com/",
                "logoUrl": "https://aura-historia.com/icon1080x1080.png",
            }),
            catalog["host"]
        );
        let entries: Vec<_> = catalog["entries"]
            .as_array()
            .unwrap()
            .iter()
            .map(|entry| {
                (
                    entry["identifier"].clone(),
                    entry["type"].clone(),
                    entry["url"].clone(),
                )
            })
            .collect();
        assert_eq!(
            vec![
                (
                    json!("urn:air:aura-historia.com:mcp:aura-historia"),
                    json!("application/mcp-server-card+json"),
                    json!("https://mcp.aura-historia.com/.well-known/mcp/server-card.json"),
                ),
                (
                    json!("urn:air:aura-historia.com:api:rest"),
                    json!("application/vnd.oai.openapi"),
                    json!("https://docs.api.aura-historia.com/swagger.yaml"),
                ),
                (
                    json!("urn:air:aura-historia.com:skills:index"),
                    json!("application/agent-skills+json"),
                    json!("https://aura-historia.com/.well-known/agent-skills/index.json"),
                ),
            ],
            entries
        );
    }

    #[test]
    fn should_give_every_entry_the_fields_registries_require() {
        for policy in [
            AuraResourcePolicy::production(),
            AuraResourcePolicy::stage(),
        ] {
            let catalog = catalog(policy);
            let domain = DiscoveryEnvironment::from(policy).site_domain();
            assert_eq!(domain, catalog["host"]["identifier"]);
            for entry in catalog["entries"].as_array().unwrap() {
                let identifier = entry["identifier"].as_str().unwrap();
                let name = identifier
                    .strip_prefix(&format!("urn:air:{domain}:"))
                    .expect("entries are published under the website domain");
                assert!(name.split(':').all(|part| {
                    !part.is_empty()
                        && part
                            .bytes()
                            .all(|byte| byte.is_ascii_alphanumeric() || b"._-".contains(&byte))
                }));
                assert!(!entry["displayName"].as_str().unwrap().is_empty());
                assert!(entry["type"].as_str().unwrap().contains('/'));
                assert!(entry["url"].is_string() && entry.get("data").is_none());
                let queries = entry["representativeQueries"].as_array().unwrap();
                assert!((2..=5).contains(&queries.len()), "{identifier}");
            }
        }
    }
}
