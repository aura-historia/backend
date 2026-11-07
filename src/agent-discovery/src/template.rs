use crate::{DiscoveryEnvironment, auth_md, site_url};
use oauth_core::ordinary_user_scopes::ORDINARY_USER_SCOPES;

/// Fills the environment's values into a Markdown document compiled into the binary. The
/// placeholders form a closed set; tests render every document for every environment and
/// reject any that remains.
pub(crate) fn render(template: &str, environment: DiscoveryEnvironment) -> String {
    let scopes = ORDINARY_USER_SCOPES
        .map(|scope| format!("`{}`", scope.as_str()))
        .join(", ");
    let auth_md_url = site_url(environment, auth_md::PATH);
    [
        ("{{site_origin}}", environment.site_origin()),
        ("{{api_origin}}", environment.api_origin()),
        ("{{issuer}}", environment.issuer()),
        ("{{mcp_origin}}", environment.mcp_origin()),
        ("{{mcp_endpoint}}", environment.mcp_endpoint()),
        (
            "{{documentation_origin}}",
            environment.documentation_origin(),
        ),
        ("{{ordinary_user_scopes}}", scopes.as_str()),
        ("{{auth_md_url}}", auth_md_url.as_str()),
    ]
    .into_iter()
    .fold(template.to_owned(), |text, (placeholder, value)| {
        text.replace(placeholder, value)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use oauth_core::resource::AuraResourcePolicy;

    #[test]
    fn should_fill_every_placeholder_with_the_environment_value() {
        let rendered = render(
            "{{site_origin}} {{api_origin}} {{issuer}} {{mcp_origin}} {{mcp_endpoint}} {{documentation_origin}} {{ordinary_user_scopes}} {{auth_md_url}}",
            AuraResourcePolicy::stage().into(),
        );
        assert_eq!(
            "https://stage.aura-historia.com https://api.stage.aura-historia.com https://api.stage.aura-historia.com https://mcp.stage.aura-historia.com https://mcp.stage.aura-historia.com/v1 https://docs.api.aura-historia.com `users:read`, `users:write`, `search-filters:read`, `search-filters:write`, `watchlist:read`, `watchlist:write`, `notifications:read`, `notifications:write` https://stage.aura-historia.com/auth.md",
            rendered
        );
    }
}
