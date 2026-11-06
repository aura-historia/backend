use credential_core::metadata_document_client_id::MetadataDocumentClientId;
use std::collections::BTreeSet;
use url::Host;

/// The operator's trust policy for clients identified by a metadata document URL.
///
/// When enabled, Aura trusts any HTTPS `client_id` whose document validates, so agents sign in
/// without a registration step. Consent shows the user the client's host, and an operator
/// denylist stops abusive hosts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MetadataDocumentClientPolicy {
    /// URL client IDs are rejected and discovery does not advertise them.
    Disabled,
    Enabled {
        denied_hosts: DeniedMetadataDocumentHosts,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum MetadataDocumentClientNotAdmitted {
    #[error("metadata document clients are disabled")]
    Disabled,
    #[error("metadata document host is denied")]
    DeniedHost,
}

impl MetadataDocumentClientPolicy {
    pub fn is_enabled(&self) -> bool {
        matches!(self, Self::Enabled { .. })
    }

    pub fn admit(
        &self,
        client_id: &MetadataDocumentClientId,
    ) -> Result<(), MetadataDocumentClientNotAdmitted> {
        match self {
            Self::Disabled => Err(MetadataDocumentClientNotAdmitted::Disabled),
            Self::Enabled { denied_hosts } if denied_hosts.denies(client_id.host()) => {
                Err(MetadataDocumentClientNotAdmitted::DeniedHost)
            }
            Self::Enabled { .. } => Ok(()),
        }
    }
}

/// Domains whose documents Aura refuses. An entry denies the domain and all its subdomains.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DeniedMetadataDocumentHosts(BTreeSet<String>);

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("denied metadata document host must be a domain name")]
pub struct InvalidDeniedMetadataDocumentHost;

impl DeniedMetadataDocumentHosts {
    pub fn parse<I, S>(entries: I) -> Result<Self, InvalidDeniedMetadataDocumentHost>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        entries
            .into_iter()
            .map(|entry| match Host::parse(entry.as_ref().trim()) {
                Ok(Host::Domain(domain)) if !domain.starts_with('.') && !domain.ends_with('.') => {
                    Ok(domain)
                }
                _ => Err(InvalidDeniedMetadataDocumentHost),
            })
            .collect::<Result<_, _>>()
            .map(Self)
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    fn denies(&self, host: &str) -> bool {
        self.0.iter().any(|denied| {
            host == denied
                || host
                    .strip_suffix(denied.as_str())
                    .is_some_and(|prefix| prefix.ends_with('.'))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn client_id(value: &str) -> MetadataDocumentClientId {
        MetadataDocumentClientId::try_from(value).unwrap()
    }

    fn enabled(denied: &[&str]) -> MetadataDocumentClientPolicy {
        MetadataDocumentClientPolicy::Enabled {
            denied_hosts: DeniedMetadataDocumentHosts::parse(denied).unwrap(),
        }
    }

    #[test]
    fn should_admit_any_https_client_when_enabled_without_a_denylist() {
        let policy = enabled(&[]);

        assert!(policy.is_enabled());
        assert_eq!(
            Ok(()),
            policy.admit(&client_id("https://claude.ai/oauth/client"))
        );
    }

    #[test]
    fn should_reject_every_client_when_disabled() {
        let policy = MetadataDocumentClientPolicy::Disabled;

        assert!(!policy.is_enabled());
        assert_eq!(
            Err(MetadataDocumentClientNotAdmitted::Disabled),
            policy.admit(&client_id("https://claude.ai/oauth/client"))
        );
    }

    #[test]
    fn should_deny_listed_domains_and_their_subdomains_only() {
        let policy = enabled(&["Abuse.Example", " spam.test "]);

        for denied in [
            "https://abuse.example/client.json",
            "https://agents.abuse.example/client.json",
            "https://spam.test/client.json",
        ] {
            assert_eq!(
                Err(MetadataDocumentClientNotAdmitted::DeniedHost),
                policy.admit(&client_id(denied)),
                "{denied}"
            );
        }
        for admitted in [
            "https://notabuse.example/client.json",
            "https://abuse.example.org/client.json",
            "https://example/client.json",
        ] {
            assert_eq!(Ok(()), policy.admit(&client_id(admitted)), "{admitted}");
        }
    }

    #[test]
    fn should_reject_denylist_entries_that_are_not_domain_names() {
        for entry in [
            "",
            "203.0.113.1",
            "[::1]",
            "https://abuse.example",
            "abuse example",
            ".abuse.example",
            "abuse.example.",
        ] {
            assert_eq!(
                Err(InvalidDeniedMetadataDocumentHost),
                DeniedMetadataDocumentHosts::parse([entry]),
                "{entry}"
            );
        }
    }
}
