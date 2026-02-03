use common::product_id::ProductId;
use product::core::{product::Product, product_event::ProductEvent};
use std::collections::HashSet;

#[derive(Debug, Clone, Default)]
pub struct ProcessResult {
    pub successes: Vec<ProductEvent>,
    pub failures: HashSet<ProductId>,
}

#[mockall::automock]
pub trait PipeProcessor {
    fn process(&self, products: Vec<Product>) -> ProcessResult;
}
