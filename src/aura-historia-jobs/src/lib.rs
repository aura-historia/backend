//! Transport-neutral compact job contract and versioned semantic SQS wire format.
pub mod jobs;
pub mod scope;
pub mod wire;

pub use jobs::*;
pub use scope::WorkerScope;
pub use wire::{MAX_JOB_BYTES, WireError, decode, encode};
