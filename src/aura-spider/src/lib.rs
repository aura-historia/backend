pub mod classification;
pub mod discovery;
pub mod domain;
pub mod error;
pub mod service;
pub mod utils;

pub use domain::{CrawledLinkMetadata, LinkClass, LinkState, SpiderRunResult};
pub use service::SpiderService;
