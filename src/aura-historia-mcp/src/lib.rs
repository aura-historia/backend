//! Bounded, stateless MCP transport. Families inject typed operations using shared contracts.
mod admission;
mod bounded_json;
mod config;
mod error;
mod http;
pub mod lambda;
mod metadata;
mod protocol;
pub mod public_discovery;
mod registry;

pub use admission::{AdmissionGate, AdmissionRejection, AdmissionRequest, AnonymousGate};
pub use config::{
    ConfigError, MAX_REQUEST_BYTES, MAX_RESPONSE_BYTES, REQUEST_TIMEOUT, TransportConfig,
};
pub use error::OperationError;
pub use http::router;
pub use metadata::InvocationContext;
pub use protocol::SUPPORTED_PROTOCOL_VERSIONS;
pub use registry::{AccessClass, OperationDefinition, OperationPolicy, Registry, RegistryError};

pub mod product_listing_history;
pub mod product_listings;
mod shipping_error;

pub mod auctions;
