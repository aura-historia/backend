//! Non-VPC DMS/Kinesis router. The legacy Sequin worker is not a dependency.
pub mod cdc;
pub mod kinesis;
pub mod queue;
mod queue_config;

pub use aura_historia_jobs::WorkerScope;
