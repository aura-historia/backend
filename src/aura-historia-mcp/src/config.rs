use oauth_core::resource::AuraResourcePolicy;
use std::{net::SocketAddr, time::Duration};
use url::Url;

pub const MAX_REQUEST_BYTES: usize = 256 * 1024;
pub const MAX_RESPONSE_BYTES: usize = 2 * 1024 * 1024;
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(25);

#[derive(Clone)]
pub struct TransportConfig {
    pub bind_addr: SocketAddr,
    pub resource_policy: AuraResourcePolicy,
    pub allowed_hosts: Vec<String>,
    pub allowed_origins: Vec<String>,
    pub max_request_bytes: usize,
    pub max_response_bytes: usize,
    pub request_timeout: Duration,
    pub max_concurrent_requests: usize,
}

impl Default for TransportConfig {
    fn default() -> Self {
        Self {
            bind_addr: SocketAddr::from(([127, 0, 0, 1], 8080)),
            resource_policy: AuraResourcePolicy::production(),
            allowed_hosts: vec!["localhost:8080".into(), "127.0.0.1:8080".into()],
            allowed_origins: Vec::new(),
            max_request_bytes: MAX_REQUEST_BYTES,
            max_response_bytes: MAX_RESPONSE_BYTES,
            request_timeout: REQUEST_TIMEOUT,
            max_concurrent_requests: 64,
        }
    }
}

impl TransportConfig {
    pub fn from_env() -> Result<Self, ConfigError> {
        let mut config = Self {
            resource_policy: AuraResourcePolicy::from_issuer(
                &env("OAUTH_ISSUER_URL")?.ok_or(ConfigError::Issuer)?,
            )
            .map_err(|_| ConfigError::Issuer)?,
            ..Default::default()
        };
        if let Some(value) = env("MCP_BIND_ADDR")? {
            config.bind_addr = value.parse().map_err(ConfigError::BindAddress)?;
        }
        if let Some(value) = env("MCP_ALLOWED_HOSTS")? {
            config.allowed_hosts = value.split(',').map(str::to_owned).collect();
        }
        if let Some(value) = env("MCP_ALLOWED_ORIGINS")? {
            config.allowed_origins = value.split(',').map(str::to_owned).collect();
        }
        config.validate()?;
        Ok(config)
    }

    pub(crate) fn validate(&self) -> Result<(), ConfigError> {
        if self.allowed_hosts.is_empty()
            || self.allowed_hosts.iter().any(|host| {
                host.is_empty()
                    || host.contains(['/', '*', '@', ',', ' '])
                    || host.parse::<axum::http::uri::Authority>().is_err()
            })
        {
            return Err(ConfigError::Hosts);
        }
        for origin in &self.allowed_origins {
            normalized_origin(origin).ok_or(ConfigError::Origins)?;
        }
        if self.max_request_bytes == 0
            || self.max_request_bytes > MAX_REQUEST_BYTES
            || self.max_response_bytes < 1024
            || self.max_response_bytes > MAX_RESPONSE_BYTES
            || self.request_timeout.is_zero()
            || self.request_timeout > REQUEST_TIMEOUT
            || self.max_concurrent_requests == 0
            || self.max_concurrent_requests > 1024
        {
            return Err(ConfigError::Limits);
        }
        Ok(())
    }
}

pub(crate) fn normalized_origin(value: &str) -> Option<String> {
    let (_, authority) = value.split_once("://")?;
    // URL parsing repairs backslashes, absent slashes and dot paths. An Origin
    // must already be a serialized scheme/authority without a path.
    if value.chars().any(char::is_whitespace) || authority.contains(['/', '\\', '?', '#']) {
        return None;
    }
    let url = Url::parse(value).ok()?;
    if !matches!(url.scheme(), "https" | "http")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.path() != "/"
        || url.query().is_some()
        || url.fragment().is_some()
        || value.ends_with('/')
    {
        return None;
    }
    Some(format!(
        "{}://{}:{}",
        url.scheme(),
        url.host_str()?,
        url.port_or_known_default()?
    ))
}

fn env(name: &'static str) -> Result<Option<String>, ConfigError> {
    match std::env::var(name) {
        Ok(value) => Ok(Some(value)),
        Err(std::env::VarError::NotPresent) => Ok(None),
        Err(_) => Err(ConfigError::Encoding(name)),
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("OAUTH_ISSUER_URL must identify the configured Aura production or stage issuer")]
    Issuer,
    #[error("invalid MCP bind address")]
    BindAddress(#[source] std::net::AddrParseError),
    #[error("MCP allowed hosts must be explicit HTTP authorities")]
    Hosts,
    #[error("MCP allowed origins must be exact HTTP origins")]
    Origins,
    #[error("MCP limits exceed the bounded transport contract")]
    Limits,
    #[error("invalid environment encoding for {0}")]
    Encoding(&'static str),
}
