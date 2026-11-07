//! The [Agent Skills Discovery](https://github.com/cloudflare/agent-skills-discovery-rfc)
//! index and the skills it lists. Each skill's Markdown is compiled into the binary; the index
//! digest is computed from exactly the bytes served for that skill.
use crate::{DiscoveryEnvironment, template};
use serde::Serialize;
use sha2::{Digest, Sha256};

pub(crate) const INDEX_SCHEMA: &str = "https://schemas.agentskills.io/discovery/0.2.0/schema.json";

struct Skill {
    name: &'static str,
    template: &'static str,
}

const SKILLS: [Skill; 3] = [
    Skill {
        name: "catalogue-search",
        template: include_str!("../documents/agent-skills/catalogue-search/SKILL.md"),
    },
    Skill {
        name: "connect-account",
        template: include_str!("../documents/agent-skills/connect-account/SKILL.md"),
    },
    Skill {
        name: "partner-listing-batches",
        template: include_str!("../documents/agent-skills/partner-listing-batches/SKILL.md"),
    },
];

/// One skill as served: its `SKILL.md` for the environment and the index entry describing it.
pub(crate) struct RenderedSkill {
    pub(crate) name: &'static str,
    pub(crate) markdown: String,
}

pub(crate) fn render_skills(environment: DiscoveryEnvironment) -> Vec<RenderedSkill> {
    SKILLS
        .iter()
        .map(|skill| RenderedSkill {
            name: skill.name,
            markdown: template::render(skill.template, environment),
        })
        .collect()
}

#[derive(Serialize)]
struct Index<'a> {
    #[serde(rename = "$schema")]
    schema: &'static str,
    skills: Vec<IndexEntry<'a>>,
}

#[derive(Serialize)]
struct IndexEntry<'a> {
    name: &'static str,
    #[serde(rename = "type")]
    kind: &'static str,
    description: &'a str,
    url: String,
    digest: String,
}

/// The index lists every skill with the URL the website publishes it at and the digest of the
/// bytes served there.
pub(crate) fn render_index(skills: &[RenderedSkill], skill_url: impl Fn(&str) -> String) -> String {
    let index = Index {
        schema: INDEX_SCHEMA,
        skills: skills
            .iter()
            .map(|skill| IndexEntry {
                name: skill.name,
                kind: "skill-md",
                description: front_matter_value(&skill.markdown, "description")
                    .expect("every compiled skill declares a description"),
                url: skill_url(skill.name),
                digest: sha256_digest(skill.markdown.as_bytes()),
            })
            .collect(),
    };
    serde_json::to_string_pretty(&index).expect("the skills index serializes")
}

/// Reads a plain scalar from the YAML front matter that opens every `SKILL.md`.
fn front_matter_value<'a>(markdown: &'a str, key: &str) -> Option<&'a str> {
    let front_matter = markdown.strip_prefix("---\n")?.split("\n---\n").next()?;
    front_matter.lines().find_map(|line| {
        line.strip_prefix(key)?
            .strip_prefix(':')
            .map(str::trim)
            .filter(|value| !value.is_empty())
    })
}

fn sha256_digest(bytes: &[u8]) -> String {
    let hex: String = Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    format!("sha256:{hex}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use oauth_core::resource::AuraResourcePolicy;

    #[test]
    fn should_open_every_skill_with_front_matter_naming_it_and_describing_its_use() {
        for skill in render_skills(AuraResourcePolicy::production().into()) {
            assert_eq!(
                Some(skill.name),
                front_matter_value(&skill.markdown, "name")
            );
            let description = front_matter_value(&skill.markdown, "description").unwrap();
            assert!(
                (1..=1024).contains(&description.chars().count()),
                "{}",
                skill.name
            );
            // A plain YAML scalar: no quoting, and no `: ` that would start a mapping.
            assert!(!description.starts_with(['"', '\'']) && !description.contains(": "));
        }
    }

    #[test]
    fn should_name_skills_as_the_agent_skills_format_requires() {
        for skill in SKILLS {
            assert!((1..=64).contains(&skill.name.len()));
            assert!(
                skill
                    .name
                    .bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
            );
            assert!(!skill.name.starts_with('-') && !skill.name.ends_with('-'));
            assert!(!skill.name.contains("--"));
        }
    }

    #[test]
    fn should_compute_the_digest_over_the_exact_bytes() {
        assert_eq!(
            "sha256:e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
            sha256_digest(b"")
        );
    }

    #[test]
    fn should_read_only_the_front_matter() {
        let markdown = "---\nname: a\ndescription: Does a.\n---\n\ndescription: not this\n";
        assert_eq!(Some("Does a."), front_matter_value(markdown, "description"));
        assert_eq!(None, front_matter_value(markdown, "license"));
        assert_eq!(None, front_matter_value("# No front matter\n", "name"));
    }
}
