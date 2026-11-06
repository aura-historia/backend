//! `auth.md`: how agents authenticate, in Markdown for agents to read. It describes anonymous
//! access, user sign-in and personal access tokens. It has no `agent_auth` block, because Aura
//! Historia registers no agent identities.
use crate::{DiscoveryEnvironment, template};

pub(crate) const PATH: &str = "auth.md";
const TEMPLATE: &str = include_str!("../documents/auth.md");

pub(crate) fn render(environment: DiscoveryEnvironment) -> String {
    template::render(TEMPLATE, environment)
}

#[cfg(test)]
mod tests {
    use super::*;
    use oauth_core::{ordinary_user_scopes::ORDINARY_USER_SCOPES, resource::AuraResourcePolicy};

    #[test]
    fn should_open_with_an_auth_md_heading_and_list_every_user_scope() {
        let rendered = render(AuraResourcePolicy::production().into());
        let heading = rendered.lines().next().unwrap();
        assert!(heading.starts_with("# ") && heading.contains("auth.md"));
        for scope in ORDINARY_USER_SCOPES {
            assert!(rendered.contains(&format!("`{}`", scope.as_str())));
        }
        assert!(!rendered.contains("agent_auth"));
    }
}
