//! Transport-neutral compact job contract and versioned semantic SQS wire format.
pub mod jobs;
pub mod scope;
pub mod wire;

pub use jobs::*;
pub use scope::{WorkerQueueType, WorkerScope};
pub use wire::{
    FifoMessageAttributes, MAX_JOB_BYTES, PreparedJob, WireError, decode, encode, prepare,
};
