use product::dynamodb::product_event_record::ProductEventRecord;
use product_pipeline_common::{
    process::{PipeProcessor, ProcessResult},
    types::InitialPipeProduct,
};
use std::collections::HashSet;
use tracing::error;

pub struct InitPipeProcessorImpl();

impl PipeProcessor<ProductEventRecord, InitialPipeProduct> for InitPipeProcessorImpl {
    fn process(&self, ins: Vec<ProductEventRecord>) -> ProcessResult<InitialPipeProduct> {
        let successes = ins
            .into_iter()
            .filter_map(|event_record| match event_record.title_native {
                Some(title_native) => Some(InitialPipeProduct {
                    product_id: event_record.product_id,
                    shop_id: event_record.shop_id,
                    shops_product_id: event_record.shops_product_id,
                    native_title: title_native,
                    native_description: event_record.description_native,
                    images: event_record.images.unwrap_or_default()
                }),
                None => {
                    error!(
                        productId = %event_record.product_id,
                        shopId = %event_record.shop_id,
                        shopsProductId = %event_record.shops_product_id,
                        eventId = %event_record.event_id,
                        "Received ProductEventRecord in intial step of product-pipeline is missing value for field 'native_title'.
                         This should not happen because the event-routing-logic in EventBridge should only route ProductEventRecords
                         of type 'CREATED' to the initial product-pipeline-queue. This is a critical logic flaw."
                    );
                    None
                }
            })
            .collect();

        ProcessResult {
            successes,
            failures: HashSet::new(),
        }
    }
}
