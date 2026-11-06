//! MCP Server Card ([SEP-2127](https://modelcontextprotocol.io/seps/2127-mcp-server-cards)):
//! pre-connection discovery built from the handler that answers `initialize` and
//! `server/discover`, so the card cannot drift from the live server identity.
use crate::{ConfigError, authentication::resource_metadata_url, protocol::ProtocolHandler};
use application::error::{box_error, static_error};
use axum::body::Bytes;
use rmcp::{
    ServerHandler,
    model::{Icon, ProtocolVersion},
};
use serde::Serialize;
use std::collections::BTreeMap;

pub(crate) const SERVER_CARD_PATH: &str = "/.well-known/mcp/server-card.json";
pub(crate) const SERVER_CARD_MEDIA_TYPE: &str = "application/mcp-server-card+json";
const SERVER_CARD_SCHEMA: &str =
    "https://static.modelcontextprotocol.io/schemas/v1/server-card.schema.json";
/// The card format carries no authorization fields; this vendor key points clients to the
/// same RFC 9728 metadata that protected-call challenges reference.
const PROTECTED_RESOURCE_METADATA_KEY: &str = "com.aura-historia/oauth-protected-resource";

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ServerCard<'a> {
    #[serde(rename = "$schema")]
    schema: &'static str,
    name: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    title: Option<&'a str>,
    version: &'a str,
    description: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    website_url: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    icons: Option<&'a [Icon]>,
    remotes: [Remote<'a>; 1],
    #[serde(rename = "_meta")]
    meta: BTreeMap<&'static str, String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Remote<'a> {
    #[serde(rename = "type")]
    transport: &'static str,
    url: &'a str,
    supported_protocol_versions: Vec<&'a str>,
}

/// Serializes the card once per router. A server identity without the card's required
/// description fails router construction instead of publishing an invalid card.
pub(crate) fn server_card(handler: &ProtocolHandler) -> Result<Bytes, ConfigError> {
    let policy = handler.resource_policy;
    let server_info = handler.get_info().server_info;
    let versions = handler.supported_protocol_versions();
    let description = server_info.description.as_deref().ok_or_else(|| {
        ConfigError::ServerCard(static_error("server identity has no description"))
    })?;
    let card = ServerCard {
        schema: SERVER_CARD_SCHEMA,
        name: &server_info.name,
        title: server_info.title.as_deref(),
        version: &server_info.version,
        description,
        website_url: server_info.website_url.as_deref(),
        icons: server_info.icons.as_deref(),
        remotes: [Remote {
            transport: "streamable-http",
            url: policy.mcp_resource(),
            supported_protocol_versions: versions.iter().map(ProtocolVersion::as_str).collect(),
        }],
        meta: BTreeMap::from([(
            PROTECTED_RESOURCE_METADATA_KEY,
            resource_metadata_url(policy),
        )]),
    };
    serde_json::to_vec(&card)
        .map(Bytes::from)
        .map_err(|source| ConfigError::ServerCard(box_error(source)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Registry, SUPPORTED_PROTOCOL_VERSIONS};
    use oauth_core::resource::AuraResourcePolicy;
    use serde_json::{Value, json};

    fn card(policy: AuraResourcePolicy) -> Value {
        let handler = ProtocolHandler {
            registry: Registry::default(),
            resource_policy: policy,
        };
        serde_json::from_slice(&server_card(&handler).unwrap()).unwrap()
    }

    fn strings(value: &Value) -> Vec<&str> {
        match value {
            Value::String(value) => vec![value],
            Value::Array(values) => values.iter().flat_map(strings).collect(),
            Value::Object(members) => members.values().flat_map(strings).collect(),
            _ => vec![],
        }
    }

    #[test]
    fn should_publish_the_runtime_version_and_protocol_versions() {
        let card = card(AuraResourcePolicy::production());
        assert_eq!(json!(env!("CARGO_PKG_VERSION")), card["version"]);
        assert_eq!(
            json!(
                SUPPORTED_PROTOCOL_VERSIONS
                    .iter()
                    .map(ProtocolVersion::as_str)
                    .collect::<Vec<_>>()
            ),
            card["remotes"][0]["supportedProtocolVersions"]
        );
    }

    #[test]
    fn should_describe_each_environment_with_its_own_urls() {
        for policy in [
            AuraResourcePolicy::production(),
            AuraResourcePolicy::stage(),
        ] {
            let site = policy.site_origin();
            let mcp = policy.mcp_origin();
            assert_eq!(
                json!({
                    "$schema": SERVER_CARD_SCHEMA,
                    "name": "com.aura-historia/mcp",
                    "title": "Aura Historia",
                    "version": env!("CARGO_PKG_VERSION"),
                    "description": "Search art and antiques from dealers and auction houses; manage your watchlist and saved searches.",
                    "websiteUrl": site,
                    "icons": [{"src": format!("{site}/icon1080x1080.png"), "mimeType": "image/png", "sizes": ["1080x1080"]}],
                    "remotes": [{
                        "type": "streamable-http",
                        "url": policy.mcp_resource(),
                        "supportedProtocolVersions": ["2026-07-28", "2025-11-25", "2025-06-18", "2025-03-26"],
                    }],
                    "_meta": {
                        "com.aura-historia/oauth-protected-resource": format!("{mcp}/.well-known/oauth-protected-resource/v1"),
                    },
                }),
                card(policy)
            );
        }
    }

    #[test]
    fn should_link_only_to_the_configured_environment() {
        for policy in [
            AuraResourcePolicy::production(),
            AuraResourcePolicy::stage(),
        ] {
            let mut card = card(policy);
            card.as_object_mut().unwrap().remove("$schema");
            let urls = strings(&card)
                .into_iter()
                .filter(|value| value.contains("://"))
                .collect::<Vec<_>>();
            assert_eq!(4, urls.len());
            for url in urls {
                assert!(
                    [policy.site_origin(), policy.mcp_origin()]
                        .iter()
                        .any(|origin| url == *origin || url.starts_with(&format!("{origin}/"))),
                    "{url} is outside the configured environment"
                );
            }
        }
    }

    #[test]
    fn should_satisfy_the_card_schema_limits() {
        let card = card(AuraResourcePolicy::production());
        let name = card["name"].as_str().unwrap();
        assert_eq!(1, name.matches('/').count());
        for field in ["title", "description"] {
            let length = card[field].as_str().unwrap().chars().count();
            assert!(
                (1..=100).contains(&length),
                "{field} has {length} characters"
            );
        }
    }
}
