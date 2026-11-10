use oauth_core::resource::AuraResourcePolicy;

/// The MCP host serves its Server Card here; the MCP crate owns that route.
const MCP_SERVER_CARD_PATH: &str = "/.well-known/mcp/server-card.json";
/// The webapp serves the website's logo here.
const SITE_LOGO_PATH: &str = "/icon1080x1080.png";

/// The origins that one environment advertises to agents. It comes from the `oauth-core`
/// resource policy that `OAUTH_ISSUER_URL` selects, so no document can mix production and
/// stage URLs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DiscoveryEnvironment {
    policy: AuraResourcePolicy,
}

impl From<AuraResourcePolicy> for DiscoveryEnvironment {
    fn from(policy: AuraResourcePolicy) -> Self {
        Self { policy }
    }
}

impl DiscoveryEnvironment {
    /// The website these documents describe, such as `https://aura-historia.com`.
    pub const fn site_origin(self) -> &'static str {
        self.policy.site_origin()
    }

    /// The website's host name, such as `aura-historia.com`.
    pub const fn site_domain(self) -> &'static str {
        self.policy.site_domain()
    }

    /// The website's logo, which the webapp serves.
    pub fn site_logo_url(self) -> String {
        format!("{}{SITE_LOGO_PATH}", self.site_origin())
    }

    /// The REST API origin; its versioned API lives below `/v1`.
    pub const fn api_origin(self) -> &'static str {
        self.policy.rest_resource()
    }

    pub const fn issuer(self) -> &'static str {
        self.policy.issuer()
    }

    pub const fn mcp_origin(self) -> &'static str {
        self.policy.mcp_origin()
    }

    /// The canonical Streamable HTTP endpoint, which is also the MCP resource identifier.
    pub const fn mcp_endpoint(self) -> &'static str {
        self.policy.mcp_resource()
    }

    pub fn mcp_server_card_url(self) -> String {
        format!("{}{MCP_SERVER_CARD_PATH}", self.mcp_origin())
    }

    pub const fn documentation_origin(self) -> &'static str {
        self.policy.documentation_origin()
    }

    /// Every origin a document of this environment may link to.
    pub const fn origins(self) -> [&'static str; 4] {
        [
            self.site_origin(),
            self.api_origin(),
            self.mcp_origin(),
            self.documentation_origin(),
        ]
    }
}
