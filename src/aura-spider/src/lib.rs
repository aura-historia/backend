pub mod classification;
pub mod crawling;
pub mod error;
pub mod spider_service;
pub mod url;

pub use spider_service::{CrawledLinkMetadata, LinkClass, SpiderRunResult, SpiderService};
