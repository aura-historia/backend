//! The trigger-fed delivery outbox: strict snapshot codecs, the dispatcher that routes captured
//! inserts into the job queue, failure custody, and full ProductListing reprojection. The
//! [README](../README.md) owns the contract.

mod custody;
mod dispatch;
mod error;
mod reprojection;
pub mod snapshot;
mod wakeup;

pub use custody::{OutboxStats, RedriveReport, RedriveSelection, inspect, redrive};
pub use dispatch::{Dispatch, DispatchReport, OutboxDispatcher};
pub use error::OutboxError;
pub use reprojection::{ReprojectionError, ReprojectionReport, reproject_all};
pub use wakeup::{OUTBOX_CHANNEL, OutboxWakeup, OutboxWakeups};
