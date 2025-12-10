use crate::{
    embed::EmbeddingDelegate,
    pipeline::pipe::{EnrichmentPipe, PipeProduct, PipeResult},
};
use common::batch::Batch;
use std::{collections::HashSet, sync::Arc};
use tracing::{error, info};

pub struct EmbeddingEnrichmentPipeImpl {
    embedding_delegate: Arc<dyn EmbeddingDelegate + Send + Sync>,
}

impl EmbeddingEnrichmentPipeImpl {
    pub fn new(embedding_delegate: Arc<dyn EmbeddingDelegate + Send + Sync>) -> Self {
        Self { embedding_delegate }
    }
}

impl EnrichmentPipe for EmbeddingEnrichmentPipeImpl {
    fn enrich(&self, products: Vec<PipeProduct>) -> PipeResult {
        let count = products.len();
        let mut successes = Vec::with_capacity(products.len());
        let mut failures = HashSet::new();
        let batches: Vec<Batch<PipeProduct, 64>> = Batch::chunked_from(products.into_iter());

        for document_batch in batches {
            let input_batch_iter = document_batch.iter().map(|pipe_product| {
                format!(
                    "{} [SEP] {}",
                    pipe_product.source.payload.native_title.payload.as_ref(),
                    pipe_product
                        .source
                        .payload
                        .native_description
                        .as_ref()
                        .map(|descr| descr.payload.as_ref())
                        .unwrap_or("")
                )
            });
            let input_batch = Batch::try_from_iter(input_batch_iter)
                .expect("shouldn't fail re-collecting former batch of same size");

            match self.embedding_delegate.embed(&input_batch) {
                Err(err) => {
                    error!(error = %err, "Failed delegating embeddings.");
                    let mut local_failed = document_batch.iter().map(|doc| doc.source.product_id);
                    failures.extend(&mut local_failed);
                }
                Ok(embeddings) => {
                    let mut local_enriched = document_batch.into_iter().zip(embeddings).map(
                        |(mut pipe_product, embedding)| {
                            pipe_product
                                .update
                                .document
                                .get_or_insert_default()
                                .text_embedding = Some(embedding);
                            pipe_product
                        },
                    );
                    successes.extend(&mut local_enriched);
                }
            }
        }

        info!(
            count = count,
            successes = count - failures.len(),
            failures = failures.len(),
            "Embedded PipeProducts."
        );

        PipeResult {
            successes,
            failures,
        }
    }
}

#[cfg(test)]
pub mod tests {
    use rstest;

    use std::sync::Arc;

    use crate::{
        embed::MockEmbeddingDelegate,
        pipeline::{
            embed::EmbeddingEnrichmentPipeImpl,
            pipe::{EnrichmentPipe, PipeProduct},
        },
    };
    use pyo3::{PyErr, exceptions::PyTypeError};

    #[test]
    fn should_keep_order_of_delegate_returned_embeddings() {
        let expected = vec![
            vec![1.234, 5.6789],
            vec![1.234, -5.6789],
            vec![-1.234, 5.6789],
            vec![-1.234, -5.6789],
        ];
        let expected_clone = expected.clone();
        let mut delegate = MockEmbeddingDelegate::default();
        delegate
            .expect_embed()
            .return_once(move |_| Ok(expected_clone.try_into().unwrap()));

        let embedding_pipe = EmbeddingEnrichmentPipeImpl::new(Arc::new(delegate));
        let res = embedding_pipe.enrich(fake::vec![PipeProduct; 4]);
        let actual = res
            .successes
            .into_iter()
            .filter_map(|doc| doc.update.document.unwrap_or_default().text_embedding)
            .collect::<Vec<_>>();

        assert_eq!(expected, actual);
        assert!(res.failures.is_empty());
    }

    #[trace]
    #[rstest::rstest]
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
        let mut delegate = MockEmbeddingDelegate::default();
        delegate
            .expect_embed()
            .returning(move |_| Err(PyErr::new::<PyTypeError, _>("Something went wrong")));

        let embedding_pipe = EmbeddingEnrichmentPipeImpl::new(Arc::new(delegate));
        let res = embedding_pipe.enrich(fake::vec![PipeProduct; input_count]);

        assert!(res.successes.is_empty());
        assert_eq!(input_count, res.failures.len());
    }
}
