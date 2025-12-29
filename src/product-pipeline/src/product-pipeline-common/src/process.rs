use common::product_id::ProductId;
use std::collections::HashSet;

#[derive(Debug, Clone, Default)]
pub struct ProcessResult<Product> {
    pub successes: Vec<Product>,
    pub failures: HashSet<ProductId>,
}

#[mockall::automock]
pub trait PipeProcessor<In, Out> {
    fn process(&self, products: Vec<In>) -> ProcessResult<Out>;
}
