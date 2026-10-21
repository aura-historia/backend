/// Exact identifiers accepted for the one logical Aura application resource.
///
/// REST and MCP are trusted transports of that resource, not separate credential audiences.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuraResource {
    ProductionRest,
    ProductionMcp,
    StageRest,
    StageMcp,
}

impl AuraResource {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ProductionRest => "https://api.aura-historia.com",
            Self::ProductionMcp => "https://mcp.aura-historia.com/v1",
            Self::StageRest => "https://api.stage.aura-historia.com",
            Self::StageMcp => "https://mcp.stage.aura-historia.com/v1",
        }
    }
}

impl TryFrom<&str> for AuraResource {
    type Error = InvalidResource;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        [
            Self::ProductionRest,
            Self::ProductionMcp,
            Self::StageRest,
            Self::StageMcp,
        ]
        .into_iter()
        .find(|resource| resource.as_str() == value)
        .ok_or(InvalidResource)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("resource is not an exact configured Aura resource identifier")]
pub struct InvalidResource;

/// The issuer and its environment-local token store bind every Aura credential,
/// including legacy credentials, to this shared logical resource. The selected
/// identifier is retained on authorization codes to prevent exchange substitution.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AuraResourcePolicy {
    rest: AuraResource,
    mcp: AuraResource,
}

impl AuraResourcePolicy {
    pub const fn production() -> Self {
        Self {
            rest: AuraResource::ProductionRest,
            mcp: AuraResource::ProductionMcp,
        }
    }

    pub const fn stage() -> Self {
        Self {
            rest: AuraResource::StageRest,
            mcp: AuraResource::StageMcp,
        }
    }

    pub fn from_issuer(issuer: &str) -> Result<Self, InvalidResource> {
        [Self::production(), Self::stage()]
            .into_iter()
            .find(|policy| policy.issuer() == issuer)
            .ok_or(InvalidResource)
    }

    pub const fn issuer(self) -> &'static str {
        self.rest.as_str()
    }

    pub const fn mcp_resource(self) -> &'static str {
        self.mcp.as_str()
    }

    pub const fn allowed_resources(self) -> [&'static str; 2] {
        [self.rest.as_str(), self.mcp.as_str()]
    }

    /// Omission retains the historical shared Aura default. Supplied identifiers
    /// are compared before URL normalization so alternate paths cannot gain trust.
    pub fn resolve(self, resource: Option<&str>) -> Result<AuraResource, InvalidResource> {
        let resource = resource
            .map(AuraResource::try_from)
            .transpose()?
            .unwrap_or(self.rest);
        if resource == self.rest || resource == self.mcp {
            Ok(resource)
        } else {
            Err(InvalidResource)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_resolve_each_environment_to_one_closed_resource_set() {
        for policy in [
            AuraResourcePolicy::production(),
            AuraResourcePolicy::stage(),
        ] {
            assert_eq!(
                policy,
                AuraResourcePolicy::from_issuer(policy.issuer()).unwrap()
            );
            assert_eq!(policy.issuer(), policy.resolve(None).unwrap().as_str());
            for resource in policy.allowed_resources() {
                assert_eq!(resource, policy.resolve(Some(resource)).unwrap().as_str());
            }
            let other = if policy == AuraResourcePolicy::production() {
                AuraResourcePolicy::stage()
            } else {
                AuraResourcePolicy::production()
            };
            for resource in other.allowed_resources() {
                assert_eq!(Err(InvalidResource), policy.resolve(Some(resource)));
            }
        }
    }

    #[test]
    fn should_reject_every_noncanonical_resource_instead_of_normalizing_it() {
        let policy = AuraResourcePolicy::production();
        for resource in [
            "",
            "https://foreign.example",
            "https://api.aura-historia.com/",
            "https://mcp.aura-historia.com",
            "https://mcp.aura-historia.com/v1/",
            "https://mcp.aura-historia.com/v1/tools",
            "https://mcp.aura-historia.com/v2",
            "https://mcp.aura-historia.com/v1#fragment",
            "https://mcp.aura-historia.com/v1?query=x",
            "https://MCP.aura-historia.com/v1",
            "https://mcp.aura-historia.com:443/v1",
            "https://user@mcp.aura-historia.com/v1",
            "http://mcp.aura-historia.com/v1",
            " https://mcp.aura-historia.com/v1",
            "https://mcp.aura-historia.com/a/../v1",
            "https://mcp.aura-historia.com/%761",
            "https://mcp.aura-historia.com.evil.example/v1",
        ] {
            assert_eq!(
                Err(InvalidResource),
                policy.resolve(Some(resource)),
                "{resource}"
            );
        }
        for issuer in [
            policy.mcp_resource(),
            "https://api.aura-historia.com/",
            "https://foreign.example",
        ] {
            assert_eq!(
                Err(InvalidResource),
                AuraResourcePolicy::from_issuer(issuer)
            );
        }
    }
}
