pub mod classification;
pub mod crawling;
pub mod error;
pub mod normalization;
pub mod spider_service;

pub use spider_service::{CrawledLinkMetadata, LinkClass, SpiderRunResult, SpiderService};
