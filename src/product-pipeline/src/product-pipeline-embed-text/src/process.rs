use crate::adapter::EmbeddingAdapter;
use common::batch::Batch;
use product::core::product::Product;
use product_pipeline_common::process::{PipeProcessor, ProcessResult};
use std::{collections::HashSet, sync::Arc};
use tracing::{error, info};

pub struct TextEmbeddingPipeProcesserImpl {
    embedding_delegate: Arc<dyn EmbeddingAdapter + Send + Sync>,
}

impl TextEmbeddingPipeProcesserImpl {
    pub fn new(embedding_delegate: Arc<dyn EmbeddingAdapter + Send + Sync>) -> Self {
        Self { embedding_delegate }
    }
}

impl PipeProcessor for TextEmbeddingPipeProcesserImpl {
    fn process(&self, ins: Vec<Product>) -> ProcessResult {
        let count = ins.len();
        let mut successes = Vec::with_capacity(ins.len());
        let mut failures = HashSet::new();
        let batches: Vec<Batch<Product, 64>> = Batch::chunked_from(ins.into_iter());

        for in_batch in batches {
            let input_batch_iter = in_batch.iter().map(|in_product| {
                format!(
                    "{} [SEP] {}",
                    in_product.native_title.payload,
                    in_product
                        .native_description
                        .as_ref()
                        .map(|descr| descr.payload.to_owned())
                        .unwrap_or("".into())
                )
            });
            let input_batch = Batch::try_from_iter(input_batch_iter)
                .expect("shouldn't fail re-collecting former batch of same size");

            match self.embedding_delegate.embed(&input_batch) {
                Err(err) => {
                    error!(error = %err, "Failed delegating embeddings.");
                    let mut local_failed = in_batch.iter().map(|in_product| in_product.product_id);
                    failures.extend(&mut local_failed);
                }
                Ok(embeddings) => {
                    let mut local_enriched = in_batch
                        .into_iter()
                        .zip(embeddings.into_iter())
                        .filter_map(|(mut product, embedding)| product.embed_text(embedding));
                    successes.extend(&mut local_enriched);
                }
            }
        }

        info!(
            count = count,
            successes = count - failures.len(),
            failures = failures.len(),
            "Text-embedded translated products."
        );

        ProcessResult {
            successes,
            failures,
        }
    }
}

#[cfg(test)]
pub mod tests {
    use crate::{adapter::MockEmbeddingAdapter, process::TextEmbeddingPipeProcesserImpl};
    use product::core::product::Product;
    use product_pipeline_common::process::PipeProcessor;
    use pyo3::{PyErr, exceptions::PyTypeError};
    use rstest;
    use std::sync::Arc;

    #[test]
    fn should_keep_order_of_delegate_returned_embeddings() {
        let expected = vec![
            vec![1.234, 5.6789],
            vec![1.234, -5.6789],
            vec![-1.234, 5.6789],
            vec![-1.234, -5.6789],
        ];
        let expected_clone = expected.clone();
        let mut delegate = MockEmbeddingAdapter::default();
        delegate
            .expect_embed()
            .return_once(move |_| Ok(expected_clone.try_into().unwrap()));

        let embedding_pipe = TextEmbeddingPipeProcesserImpl::new(Arc::new(delegate));
        let res = embedding_pipe.process(fake::vec![Product; 4]);
        let actual = res
            .successes
            .into_iter()
            .map(|event| event.payload.as_embedded_text().unwrap().embedding.clone())
            .collect::<Vec<_>>();

        assert_eq!(expected, actual);
        assert!(res.failures.is_empty());
    }

    #[rstest::rstest]
    #[trace]
    #[case(1)]
    #[case(5)]
    #[case(20)]
    #[case(32)]
    #[case(64)]
    #[case(70)]
    #[case(128)]
    #[case(129)]
    #[case(256)]
    #[case(1000)]
    #[case(1500)]
    fn should_partially_fail_entire_batches(#[case] input_count: usize) {
        let mut delegate = MockEmbeddingAdapter::default();
        delegate
            .expect_embed()
            .returning(move |_| Err(PyErr::new::<PyTypeError, _>("Something went wrong")));

        let embedding_pipe = TextEmbeddingPipeProcesserImpl::new(Arc::new(delegate));
        let res = embedding_pipe.process(fake::vec![Product; input_count]);

        assert!(res.successes.is_empty());
        assert_eq!(input_count, res.failures.len());
    }
}
