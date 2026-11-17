//! Durable PostgreSQL job queue: scoped, leased, at-least-once delivery with FIFO groups,
//! optional deduplication and failure custody. The [README](../README.md) owns the contract.

mod custody;
mod enqueue;
mod error;
mod job;
mod queue;
mod scope;
mod wakeup;

pub use custody::{CustodyReason, RedriveReport, RedriveSelection};
pub use enqueue::{EnqueueOutcome, enqueue};
pub use error::JobQueueError;
pub use job::{
    ClaimedJob, DedupKey, ErrorClass, GroupKey, JobError, JobId, JobLease, MAX_PAYLOAD_BYTES,
    NewJob,
};
pub use queue::{Disposition, JobQueue, ScopeStats, Settlement};
pub use scope::{JobScope, QueueOrdering, ScopeError, ScopeName};
pub use wakeup::{JobWakeups, Wakeup};
