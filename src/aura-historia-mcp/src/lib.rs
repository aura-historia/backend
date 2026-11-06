//! Bounded, stateless MCP transport. Families inject typed operations using shared contracts.
mod admission;
mod arguments;
mod authentication;
mod bounded_json;
mod config;
mod error;
mod http;
pub mod lambda;
mod metadata;
mod protocol;
pub mod public;
pub mod public_discovery;
mod registry;
mod runtime;
mod server_card;
mod server_info;

pub use admission::{AdmissionGate, AdmissionRejection, AdmissionRequest, AnonymousGate};
pub use authentication::AuraTokenGate;
pub use config::{
    ConfigError, MAX_REQUEST_BYTES, MAX_RESPONSE_BYTES, REQUEST_TIMEOUT, TransportConfig,
    UNRELEASED_SERVER_VERSION,
};
pub use error::OperationError;
pub use http::router;
pub use metadata::InvocationContext;
pub use protocol::SUPPORTED_PROTOCOL_VERSIONS;
pub use registry::{
    AccessClass, Completed, OperationDefinition, OperationPolicy, Registry, RegistryError,
};
pub use runtime::{LambdaMcpRouter, McpDependencies, McpStartupError, mcp_router};

pub mod product_listing_history;
pub mod product_listings;
mod shipping_error;

pub mod auctions;

pub mod tools;
