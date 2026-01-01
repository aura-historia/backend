use product_pipeline_common::{
    process::{PipeProcessor, ProcessResult},
    types::{AttributeExtractedPipeProduct, CompletedPipeProduct},
};
use std::collections::HashSet;

pub struct CompleterPipeProcessorImpl();
impl PipeProcessor<AttributeExtractedPipeProduct, CompletedPipeProduct>
    for CompleterPipeProcessorImpl
{
    fn process(
        &self,
        ins: Vec<AttributeExtractedPipeProduct>,
    ) -> ProcessResult<CompletedPipeProduct> {
        let successes = ins
            .into_iter()
            .map(|in_product| CompletedPipeProduct {
                product_id: in_product.product_id,
                shop_id: in_product.shop_id,
                shops_product_id: in_product.shops_product_id,
                native_title: in_product.native_title,
                other_title: in_product.other_title,
                native_description: in_product.native_description,
                other_description: in_product.other_description,
                origin_year_min: in_product.origin_year_min,
                origin_year: in_product.origin_year,
                origin_year_max: in_product.origin_year_max,
                authenticity: Some(in_product.authenticity),
                condition: Some(in_product.condition),
                provenance: Some(in_product.provenance),
                restoration: Some(in_product.restoration),
                text_embedding: in_product.text_embedding,
            })
            .collect();
        ProcessResult {
            successes,
            failures: HashSet::new(),
        }
    }
}
