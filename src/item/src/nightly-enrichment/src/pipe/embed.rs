use crate::{
    embed::EmbeddingDelegate,
    pipe::spec::{EnrichmentPipe, PipeItem},
};
use common::batch::Batch;
use pyo3::PyErr;

pub struct EmbeddingEnrichmentPipeImpl<'a> {
    embedding_delegate: &'a dyn EmbeddingDelegate,
}

impl<'a> EmbeddingEnrichmentPipeImpl<'a> {
    pub fn new(embedding_delegate: &'a dyn EmbeddingDelegate) -> Self {
        Self { embedding_delegate }
    }
}

impl<'a> EnrichmentPipe for EmbeddingEnrichmentPipeImpl<'a> {
    type Error = PyErr;

    fn enrich(&self, items: Vec<PipeItem>) -> Result<Vec<PipeItem>, PyErr> {
        let mut enriched = Vec::with_capacity(items.len());
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

            let embeddings = self.embedding_delegate.embed(&input_batch)?;
            let mut local_enriched =
                document_batch
                    .into_iter()
                    .zip(embeddings)
                    .map(|(mut pipe_item, embedding)| {
                        pipe_item.update.document.get_or_insert_default().embedding =
                            Some(embedding);
                        pipe_item
                    });
            enriched.extend(&mut local_enriched);
        }

        Ok(enriched)
    }
}

#[cfg(test)]
pub mod tests {
    use crate::{
        embed::MockEmbeddingDelegate,
        pipe::{
            embed::EmbeddingEnrichmentPipeImpl,
            spec::{EnrichmentPipe, PipeItem},
        },
    };

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

        let embedding_pipe = EmbeddingEnrichmentPipeImpl::new(&delegate);
        let actual = embedding_pipe
            .enrich(fake::vec![PipeItem; 42])
            .unwrap()
            .into_iter()
            .filter_map(|doc| doc.update.document.unwrap_or_default().embedding)
            .collect::<Vec<_>>();

        assert_eq!(expected, actual);
    }
}
