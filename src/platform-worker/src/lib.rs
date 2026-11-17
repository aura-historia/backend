//! The long-running consumer of the PostgreSQL job queue: one claim loop per scope with a fixed
//! concurrency, a per-job timeout, panic containment, wake-ups with a polling fallback and a
//! graceful shutdown. Job semantics stay with each scope's [`JobHandler`]; the
//! [README](../README.md) owns the contract.

mod consumer;
mod error_class;
mod job;
mod wakeup;

pub use consumer::{Consumer, ConsumerConfig, ConsumerConfigError, JobHandler};
pub use error_class::error_class;
pub use job::Job;
pub use wakeup::ScopeWakeups;
