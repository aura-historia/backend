//! The server identity that `initialize` and `server/discover` report and directories display.
use crate::TransportConfig;
use oauth_core::resource::AuraResourcePolicy;
use rmcp::model::{Icon, Implementation};

/// Programmatic identifier; clients may persist it, so it stays stable.
const SERVER_NAME: &str = "aura-historia-mcp";
const SERVER_TITLE: &str = "Aura Historia";
const SERVER_DESCRIPTION: &str = "Search Aura Historia's index of art, antiques and design objects listed by dealers, auction houses, marketplaces and auction platforms, and manage the signed-in user's account, watchlist, saved searches and notifications.";

pub(crate) fn server_info(config: &TransportConfig) -> Implementation {
    let website = config.resource_policy.site_origin();
    Implementation::new(SERVER_NAME, config.server_version.as_str())
        .with_title(SERVER_TITLE)
        .with_description(SERVER_DESCRIPTION)
        .with_icons(vec![
            png_icon(website, "icon1080x1080.png", "1080x1080"),
            png_icon(website, "favicon.png", "32x32"),
        ])
        .with_website_url(website)
}

fn png_icon(website: &str, file: &str, size: &str) -> Icon {
    Icon::new(format!("{website}/{file}"))
        .with_mime_type("image/png")
        .with_sizes(vec![size.to_owned()])
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn should_describe_each_environment_with_its_own_website_and_release() {
        for (policy, website) in [
            (
                AuraResourcePolicy::production(),
                "https://aura-historia.com",
            ),
            (
                AuraResourcePolicy::stage(),
                "https://stage.aura-historia.com",
            ),
        ] {
            let config = TransportConfig {
                resource_policy: policy,
                server_version: "0123456789abcdef0123456789abcdef01234567".into(),
                ..Default::default()
            };
            assert_eq!(
                json!({
                    "name": "aura-historia-mcp",
                    "title": "Aura Historia",
                    "version": "0123456789abcdef0123456789abcdef01234567",
                    "description": SERVER_DESCRIPTION,
                    "icons": [
                        {
                            "src": format!("{website}/icon1080x1080.png"),
                            "mimeType": "image/png",
                            "sizes": ["1080x1080"],
                        },
                        {
                            "src": format!("{website}/favicon.png"),
                            "mimeType": "image/png",
                            "sizes": ["32x32"],
                        },
                    ],
                    "websiteUrl": website,
                }),
                serde_json::to_value(server_info(&config)).unwrap()
            );
        }
    }
}
