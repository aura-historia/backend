//! Router-only entry point to the shared read-only Standard SQS destination contract.
#[cfg(test)]
#[path = "queue_config_tests.rs"]
mod tests;

#[cfg(test)]
use platform_worker_queue_contract::Attributes;
pub use platform_worker_queue_contract::{
    CdcRouterQueueConfig, QueueError, SqsQueueConfig, validate_attributes,
};
