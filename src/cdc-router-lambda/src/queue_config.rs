//! Router-only entry point to the shared read-only Standard/FIFO SQS destination contract.
pub use platform_worker_queue_contract::{
    CdcRouterQueueConfig, QueueError, SqsQueueConfig, validate_attributes,
};
