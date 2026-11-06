//! The [Personal Agent Protocol](https://personalagentprotocol.org/) declaration. It declares
//! the website only (`web: {}`), with the protocol version that agent-readiness scanners
//! validate. Declaring `apis` would require the issuer to bind this domain in its metadata
//! (`poppy_domains`) and every listed API to accept Poppy session tokens; Aura Historia does
//! neither, so agents browse the website as signed-out visitors.
use crate::DiscoveryEnvironment;
use serde::Serialize;

pub(crate) const PATH: &str = "poppy.json";
const PROTOCOL_VERSION: &str = "0.1";

#[derive(Serialize)]
struct Poppy {
    protocol_version: &'static str,
    organization: Organization,
    web: Web,
}

/// `domain` must equal the host the declaration is served from.
#[derive(Serialize)]
struct Organization {
    name: &'static str,
    domain: &'static str,
}

#[derive(Serialize)]
struct Web {}

pub(crate) fn render(environment: DiscoveryEnvironment) -> String {
    let poppy = Poppy {
        protocol_version: PROTOCOL_VERSION,
        organization: Organization {
            name: "Aura Historia",
            domain: environment.site_domain(),
        },
        web: Web {},
    };
    serde_json::to_string_pretty(&poppy).expect("the Poppy declaration serializes")
}

#[cfg(test)]
mod tests {
    use super::*;
    use oauth_core::resource::AuraResourcePolicy;
    use serde_json::{Value, json};

    #[test]
    fn should_declare_the_website_of_each_environment_without_apis_or_auth() {
        for (policy, domain) in [
            (AuraResourcePolicy::production(), "aura-historia.com"),
            (AuraResourcePolicy::stage(), "stage.aura-historia.com"),
        ] {
            let environment = DiscoveryEnvironment::from(policy);
            let poppy: Value = serde_json::from_str(&render(environment)).unwrap();
            assert_eq!(
                json!({
                    "protocol_version": "0.1",
                    "organization": {"name": "Aura Historia", "domain": domain},
                    "web": {},
                }),
                poppy
            );
            assert_eq!(
                environment.site_origin(),
                format!(
                    "https://{}",
                    poppy["organization"]["domain"].as_str().unwrap()
                )
            );
        }
    }
}
