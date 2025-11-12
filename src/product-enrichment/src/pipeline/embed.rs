use std::{collections::HashSet, sync::Arc};

use crate::{
    embed::EmbeddingDelegate,
    pipeline::pipe::{EnrichmentPipe, PipeItem, PipeResult},
};
use common::batch::Batch;
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
    fn enrich(&self, items: Vec<PipeItem>) -> PipeResult {
        let count = items.len();
        let mut successes = Vec::with_capacity(items.len());
        let mut failures = HashSet::new();
        let batches: Vec<Batch<PipeItem, 64>> = Batch::chunked_from(items.into_iter());

        for document_batch in batches {
            let input_batch_iter = document_batch.iter().map(|pipe_item| {
                format!(
                    "{} [SEP] {}",
                    pipe_item.source.payload.native_title.payload.as_ref(),
                    pipe_item
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
                        |(mut pipe_item, embedding)| {
                            pipe_item
                                .update
                                .document
                                .get_or_insert_default()
                                .text_embedding = Some(embedding);
                            pipe_item
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
            "Embedded PipeItems."
        );

        PipeResult {
            successes,
            failures,
        }
    }
}

#[cfg(test)]
pub mod tests {
    use std::sync::Arc;

    use crate::{
        embed::MockEmbeddingDelegate,
        pipeline::{
            embed::EmbeddingEnrichmentPipeImpl,
            pipe::{EnrichmentPipe, PipeItem},
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
        let res = embedding_pipe.enrich(fake::vec![PipeItem; 4]);
        let actual = res
            .successes
            .into_iter()
            .filter_map(|doc| doc.update.document.unwrap_or_default().text_embedding)
            .collect::<Vec<_>>();

        assert_eq!(expected, actual);
        assert!(res.failures.is_empty());
    }

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
        let res = embedding_pipe.enrich(fake::vec![PipeItem; input_count]);

        assert!(res.successes.is_empty());
        assert_eq!(input_count, res.failures.len());
    }
}
