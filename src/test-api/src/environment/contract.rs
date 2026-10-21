use super::{Result, context, invalid};
use serde::{Deserialize, Serialize};
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use url::Url;

pub(super) const MANAGED: &str = "AURA_TEST_ENV_MANAGED";
pub(super) const CONFIG: &str = "AURA_TEST_ENV_CONFIG";

/// Validated immutable endpoints for one supervisor invocation. This is not a test lease.
#[derive(Debug, Clone, Serialize)]
pub struct ManagedEnvironment {
    pub(crate) schema_version: u32,
    pub(crate) run_id: String,
    pub(crate) profile: Profile,
    pub(crate) slots: u16,
    pub(crate) postgres: Option<PostgresEndpoint>,
    pub(crate) aws_endpoint: Option<String>,
    pub(crate) opensearch_endpoint: Option<String>,
    pub(crate) aws_services: Vec<String>,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub(crate) enum Profile {
    Postgres,
    Aws,
    Opensearch,
    Api,
}

impl Profile {
    pub(crate) fn postgres(self) -> bool {
        matches!(self, Self::Postgres | Self::Api)
    }
    pub(crate) fn aws(self) -> bool {
        !matches!(self, Self::Postgres)
    }
    pub(crate) fn opensearch(self) -> bool {
        matches!(self, Self::Opensearch | Self::Api)
    }
    pub(crate) fn services(self) -> Vec<String> {
        if !self.aws() {
            return vec![];
        }
        let mut services = vec!["eventbridge", "s3", "sesv2", "sqs"];
        if self.opensearch() {
            services.push("opensearch");
        }
        services.sort_unstable();
        services.into_iter().map(str::to_owned).collect()
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct PostgresEndpoint {
    pub(crate) port: u16,
    pub(crate) root_certificate: PathBuf,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WireEnvironment {
    schema_version: u32,
    run_id: String,
    profile: Profile,
    slots: u16,
    postgres: Option<PostgresEndpoint>,
    aws_endpoint: Option<String>,
    opensearch_endpoint: Option<String>,
    aws_services: Vec<String>,
}

impl ManagedEnvironment {
    /// Deserialize through validation; unknown versions and nonlocal endpoints fail closed.
    pub fn from_json(json: &[u8]) -> Result<Self> {
        let wire: WireEnvironment = serde_json::from_slice(json)
            .map_err(|e| context("invalid managed environment JSON", e))?;
        let value = Self {
            schema_version: wire.schema_version,
            run_id: wire.run_id,
            profile: wire.profile,
            slots: wire.slots,
            postgres: wire.postgres,
            aws_endpoint: wire.aws_endpoint,
            opensearch_endpoint: wire.opensearch_endpoint,
            aws_services: wire.aws_services,
        };
        value.validate()?;
        Ok(value)
    }

    pub fn run_id(&self) -> &str {
        &self.run_id
    }
    pub fn slots(&self) -> u16 {
        self.slots
    }
    pub fn aws_endpoint(&self) -> Option<&str> {
        self.aws_endpoint.as_deref()
    }
    pub fn opensearch_endpoint(&self) -> Option<&str> {
        self.opensearch_endpoint.as_deref()
    }

    pub(crate) fn validate(&self) -> Result<()> {
        if self.schema_version != 1
            || !valid_run_id(&self.run_id)
            || !(1..=1024).contains(&self.slots)
        {
            return Err(invalid(
                "unsupported managed version, run identity or slot count",
            ));
        }
        if self.profile.postgres() != self.postgres.is_some()
            || self.profile.aws() != self.aws_endpoint.is_some()
            || self.profile.opensearch() != self.opensearch_endpoint.is_some()
            || self.aws_services != self.profile.services()
        {
            return Err(invalid("managed topology does not match its profile"));
        }
        for endpoint in [&self.aws_endpoint, &self.opensearch_endpoint]
            .into_iter()
            .flatten()
        {
            validate_endpoint(endpoint)?;
        }
        if let Some(pg) = &self.postgres {
            if pg.port == 0 || !pg.root_certificate.is_absolute() || !pg.root_certificate.is_file()
            {
                return Err(invalid(
                    "managed Postgres requires a port and readable absolute CA file",
                ));
            }
            std::fs::File::open(&pg.root_certificate)
                .map_err(|e| context("cannot read managed Postgres CA", e))?;
        }
        Ok(())
    }

    pub(crate) fn assert_aws_topology(&self, services: &[&str], env: &[(&str, &str)]) {
        assert!(
            self.aws_endpoint.is_some(),
            "managed profile has no AWS emulator"
        );
        assert!(env.is_empty(), "managed Floci environment is immutable");
        assert!(
            services
                .iter()
                .all(|s| self.aws_services.iter().any(|v| v == s)),
            "requested AWS service is absent from managed topology"
        );
    }
}

pub(super) fn valid_run_id(run: &str) -> bool {
    run.len() == 32
        && run
            .bytes()
            .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
}

fn validate_endpoint(endpoint: &str) -> Result<()> {
    let url = Url::parse(endpoint).map_err(|e| context("invalid managed endpoint URL", e))?;
    if url.scheme() != "http"
        || url.host_str() != Some("127.0.0.1")
        || url.port().is_none()
        || url.port() == Some(0)
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || url.path() != "/"
    {
        return Err(invalid(
            "managed service endpoint must be a loopback HTTP origin with explicit port",
        ));
    }
    Ok(())
}

fn load(flag: Option<&str>, path: Option<&Path>) -> Result<Option<ManagedEnvironment>> {
    match (flag, path) {
        (None, None) => Ok(None),
        (Some("1"), Some(path)) if path.is_absolute() => {
            let bytes = std::fs::read(path)
                .map_err(|e| context("cannot read managed environment file", e))?;
            let config = ManagedEnvironment::from_json(&bytes)?;
            let dir = path
                .parent()
                .ok_or_else(|| invalid("managed configuration has no run directory"))?;
            let meta = std::fs::symlink_metadata(dir)?;
            if !meta.is_dir()
                || meta.uid() != unsafe { libc::geteuid() }
                || meta.permissions().mode() & 0o077 != 0
                || dir.file_name().and_then(|v| v.to_str()) != Some(config.run_id())
                || path.file_name().and_then(|v| v.to_str()) != Some("environment.json")
                || std::fs::symlink_metadata(path)?.file_type().is_symlink()
                || config
                    .postgres
                    .as_ref()
                    .is_some_and(|pg| pg.root_certificate.parent() != Some(dir))
            {
                return Err(invalid(
                    "managed configuration must belong to its private run directory",
                ));
            }
            Ok(Some(config))
        }
        _ => Err(invalid(
            "managed mode requires AURA_TEST_ENV_MANAGED=1 and an absolute AURA_TEST_ENV_CONFIG",
        )),
    }
}

/// Load once before constructing fixtures. Partial configuration never enables unmanaged fallback.
pub fn managed_environment() -> Option<&'static ManagedEnvironment> {
    static ENV: OnceLock<Option<ManagedEnvironment>> = OnceLock::new();
    ENV.get_or_init(|| {
        let flag = std::env::var_os(MANAGED);
        let path = std::env::var_os(CONFIG);
        let flag = flag
            .as_ref()
            .map(|s| s.to_str().expect("managed flag must be UTF-8"));
        load(flag, path.as_deref().map(Path::new))
            .unwrap_or_else(|e| panic!("invalid managed test environment: {e}"))
    })
    .as_ref()
}

#[cfg(test)]
mod tests {
    use super::*;
    fn aws() -> serde_json::Value {
        serde_json::json!({"schema_version":1,"run_id":"0123456789abcdef0123456789abcdef",
            "profile":"AWS","slots":4,"postgres":null,"aws_endpoint":"http://127.0.0.1:4567",
            "opensearch_endpoint":null,"aws_services":["eventbridge","s3","sesv2","sqs"]})
    }
    #[test]
    fn should_validate_versioned_topology_without_inheriting_implicit_services() {
        let value = ManagedEnvironment::from_json(&serde_json::to_vec(&aws()).unwrap()).unwrap();
        assert_eq!(4, value.slots());
        value.assert_aws_topology(&["sqs"], &[]);
        assert!(
            std::panic::catch_unwind(|| value.assert_aws_topology(&["opensearch"], &[])).is_err()
        );
    }
    #[test]
    fn should_reject_incomplete_managed_markers() {
        assert!(load(None, None).unwrap().is_none());
        for flag in [None, Some("0"), Some("true"), Some("1")] {
            assert!(load(flag, Some(Path::new("relative.json"))).is_err());
        }
        assert!(load(Some("1"), None).is_err());
    }
    #[test]
    fn should_reject_invalid_contracts_and_external_endpoints() {
        for (field, value) in [
            ("schema_version", serde_json::json!(2)),
            ("slots", serde_json::json!(0)),
            ("run_id", serde_json::json!("../foreign")),
            ("unknown", serde_json::json!(true)),
            (
                "postgres",
                serde_json::json!({"port":5432,"root_certificate":"/tmp/absent-ca"}),
            ),
        ] {
            let mut v = aws();
            v[field] = value;
            assert!(ManagedEnvironment::from_json(&serde_json::to_vec(&v).unwrap()).is_err());
        }
        for url in [
            "http://example.com:1234",
            "http://localhost:1234",
            "http://127.0.0.1",
            "http://127.0.0.1:0",
            "http://user:password@127.0.0.1:1234",
            "http://127.0.0.1:1234/path",
            "http://127.0.0.1:1234?x=1",
        ] {
            let mut v = aws();
            v["aws_endpoint"] = url.into();
            assert!(ManagedEnvironment::from_json(&serde_json::to_vec(&v).unwrap()).is_err());
        }
    }
}
