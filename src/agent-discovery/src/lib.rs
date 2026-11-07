//! The documents that describe the website to agents. The backend writes them and the API
//! serves them below `/v1/site-discovery/`; the webapp publishes each at its website path.
//! Rendering is pure: every value comes from the [`DiscoveryEnvironment`] and from Markdown
//! compiled into the binary. The [agent discovery contract](../../../docs/agent-discovery.md)
//! owns the shapes.
mod agent_skills;
mod ai_catalog;
mod auth_md;
mod environment;
mod poppy;
mod template;

pub use environment::DiscoveryEnvironment;

const JSON: &str = "application/json";
const MARKDOWN: &str = "text/markdown; charset=utf-8";
const SKILLS_INDEX_PATH: &str = "agent-skills/index.json";

/// One rendered document, addressed by its path below the discovery root.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SiteDocument {
    path: String,
    media_type: &'static str,
    body: String,
}

impl SiteDocument {
    fn new(path: impl Into<String>, media_type: &'static str, body: String) -> Self {
        Self {
            path: path.into(),
            media_type,
            body,
        }
    }

    /// The path below the discovery root, such as `agent-skills/index.json`.
    pub fn path(&self) -> &str {
        &self.path
    }

    /// Where the website publishes this document, such as `/.well-known/agent-skills/index.json`.
    pub fn site_path(&self) -> String {
        site_path(&self.path)
    }

    pub fn media_type(&self) -> &'static str {
        self.media_type
    }

    pub fn body(&self) -> &str {
        &self.body
    }
}

/// Every website discovery document of one environment, rendered once.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SiteDocuments {
    documents: Vec<SiteDocument>,
}

impl SiteDocuments {
    pub fn render(environment: DiscoveryEnvironment) -> Self {
        let skills = agent_skills::render_skills(environment);
        let skills_index =
            agent_skills::render_index(&skills, |name| site_url(environment, &skill_path(name)));
        let mut documents = vec![
            SiteDocument::new(
                ai_catalog::PATH,
                ai_catalog::MEDIA_TYPE,
                ai_catalog::render(environment),
            ),
            SiteDocument::new(poppy::PATH, JSON, poppy::render(environment)),
            SiteDocument::new(SKILLS_INDEX_PATH, JSON, skills_index),
            SiteDocument::new(auth_md::PATH, MARKDOWN, auth_md::render(environment)),
        ];
        documents.extend(
            skills
                .into_iter()
                .map(|skill| SiteDocument::new(skill_path(skill.name), MARKDOWN, skill.markdown)),
        );
        Self { documents }
    }

    /// The document at a path below the discovery root; `None` for any other path.
    pub fn get(&self, path: &str) -> Option<&SiteDocument> {
        self.documents.iter().find(|document| document.path == path)
    }

    pub fn iter(&self) -> impl Iterator<Item = &SiteDocument> {
        self.documents.iter()
    }
}

fn skill_path(name: &str) -> String {
    format!("agent-skills/{name}/SKILL.md")
}

/// Documents that agents look for at their own path below the website root rather than below
/// `/.well-known`.
const ROOT_DOCUMENTS: [&str; 1] = [auth_md::PATH];

/// Where the website publishes the document at `path` below the discovery root.
fn site_path(path: &str) -> String {
    if ROOT_DOCUMENTS.contains(&path) {
        format!("/{path}")
    } else {
        format!("/.well-known/{path}")
    }
}

fn site_url(environment: DiscoveryEnvironment, path: &str) -> String {
    format!("{}{}", environment.site_origin(), site_path(path))
}

#[cfg(test)]
mod tests {
    use super::*;
    use oauth_core::resource::AuraResourcePolicy;
    use sha2::{Digest, Sha256};

    fn environments() -> [DiscoveryEnvironment; 2] {
        [
            AuraResourcePolicy::production().into(),
            AuraResourcePolicy::stage().into(),
        ]
    }

    /// Absolute URLs in a document's text, without surrounding Markdown or JSON punctuation.
    fn urls(text: &str) -> Vec<&str> {
        text.match_indices("http")
            .filter(|(start, _)| {
                text[*start..].starts_with("https://") || text[*start..].starts_with("http://")
            })
            .map(|(start, _)| {
                let rest = &text[start..];
                let end = rest
                    .find(|c: char| c.is_whitespace() || "`\"'()<>[]|,".contains(c))
                    .unwrap_or(rest.len());
                rest[..end].trim_end_matches(['.', ':', ';'])
            })
            .collect()
    }

    #[test]
    fn should_link_only_to_the_environment_origins_apart_from_schema_identifiers() {
        for environment in environments() {
            let documents = SiteDocuments::render(environment);
            let mut checked = 0;
            for document in documents.iter() {
                for url in urls(document.body()) {
                    if url == agent_skills::INDEX_SCHEMA {
                        continue;
                    }
                    assert!(
                        environment
                            .origins()
                            .iter()
                            .any(|origin| url == *origin || url.starts_with(&format!("{origin}/"))),
                        "{url} in {} is outside the environment",
                        document.path()
                    );
                    checked += 1;
                }
            }
            assert!(checked > 20, "{checked}");
        }
    }

    #[test]
    fn should_leave_no_placeholder_unrendered() {
        for environment in environments() {
            for document in SiteDocuments::render(environment).iter() {
                assert!(!document.body().contains("{{"), "{}", document.path());
                assert!(!document.body().contains("}}"), "{}", document.path());
            }
        }
    }

    /// The document the website publishes at a URL.
    fn published<'a>(
        documents: &'a SiteDocuments,
        environment: DiscoveryEnvironment,
        url: &str,
    ) -> Option<&'a SiteDocument> {
        documents.iter().find(|document| {
            url == format!("{}{}", environment.site_origin(), document.site_path())
        })
    }

    #[test]
    fn should_publish_auth_md_at_the_website_root_and_the_rest_below_well_known() {
        let documents = SiteDocuments::render(AuraResourcePolicy::production().into());
        let paths: Vec<_> = documents.iter().map(SiteDocument::site_path).collect();
        assert_eq!(
            vec![
                "/.well-known/ai-catalog.json",
                "/.well-known/poppy.json",
                "/.well-known/agent-skills/index.json",
                "/auth.md",
                "/.well-known/agent-skills/catalogue-search/SKILL.md",
                "/.well-known/agent-skills/connect-account/SKILL.md",
                "/.well-known/agent-skills/partner-listing-batches/SKILL.md",
            ],
            paths
        );
    }

    #[test]
    fn should_publish_skill_digests_of_the_bytes_served_at_each_listed_url() {
        for environment in environments() {
            let documents = SiteDocuments::render(environment);
            let index: serde_json::Value =
                serde_json::from_str(documents.get(SKILLS_INDEX_PATH).unwrap().body()).unwrap();
            let entries = index["skills"].as_array().unwrap();
            assert_eq!(3, entries.len());
            for entry in entries {
                let url = entry["url"].as_str().unwrap();
                let served = published(&documents, environment, url)
                    .expect("every listed skill is published on the website");
                assert_eq!(MARKDOWN, served.media_type());
                let hex: String = Sha256::digest(served.body().as_bytes())
                    .iter()
                    .map(|byte| format!("{byte:02x}"))
                    .collect();
                assert_eq!(format!("sha256:{hex}"), entry["digest"]);
            }
        }
    }

    #[test]
    fn should_resolve_every_website_link_to_a_published_document() {
        for environment in environments() {
            let documents = SiteDocuments::render(environment);
            let site = environment.site_origin();
            for document in documents.iter() {
                for url in urls(document.body()) {
                    // The home page and the logo are the webapp's own.
                    if url.starts_with(site) && url != site && url != environment.site_logo_url() {
                        assert!(
                            published(&documents, environment, url).is_some(),
                            "{url} in {}",
                            document.path()
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn should_serve_each_document_as_its_format_requires() {
        for document in SiteDocuments::render(AuraResourcePolicy::stage().into()).iter() {
            if document.path().ends_with(".json") {
                assert!(
                    document.media_type().ends_with("json"),
                    "{}",
                    document.path()
                );
                serde_json::from_str::<serde_json::Value>(document.body())
                    .unwrap_or_else(|error| panic!("{}: {error}", document.path()));
            } else {
                assert!(document.path().ends_with(".md"));
                assert_eq!(MARKDOWN, document.media_type());
            }
        }
    }
}
