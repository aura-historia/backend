//! Public wire values shared by REST and MCP adapters.
//!
//! Pure DTO conversions belong here. HTTP responses, extractors, headers and errors
//! remain in the adapter that owns the transport.

pub mod patch_value;
pub mod values;
pub mod wire;
