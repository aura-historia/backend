use product_pipeline_common::{
    process::{PipeProcessor, ProcessResult},
    types::{CompletedPipeProduct, TextEmbeddedPipeProduct},
};
use std::collections::HashSet;

pub struct CompleterPipeProcessorImpl();
impl PipeProcessor<TextEmbeddedPipeProduct, CompletedPipeProduct> for CompleterPipeProcessorImpl {
    fn process(&self, ins: Vec<TextEmbeddedPipeProduct>) -> ProcessResult<CompletedPipeProduct> {
        let successes = ins
            .into_iter()
            .map(|in_product| CompletedPipeProduct {
                product_id: in_product.product_id,
                shop_id: in_product.shop_id,
                shops_product_id: in_product.shops_product_id,
                shop_name: in_product.shop_name,
                native_title: in_product.native_title,
                other_title: in_product.other_title,
                native_description: in_product.native_description,
                other_description: in_product.other_description,
                text_embedding: in_product.text_embedding,
            })
            .collect();
        ProcessResult {
            successes,
            failures: HashSet::new(),
        }
    }
}
