use crate::{embed::EmbeddingDelegate, pipe::spec::EnrichmentPipe};
use common::batch::Batch;
use item_opensearch::item_document::ItemDocument;
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

    fn enrich(&self, items: Vec<ItemDocument>) -> Result<Vec<ItemDocument>, PyErr> {
        let mut enriched = Vec::with_capacity(items.len());
        let batches: Vec<Batch<ItemDocument, 64>> = Batch::chunked_from(items.into_iter());

        for document_batch in batches {
            let input_batch_iter = document_batch.iter().map(|doc| {
                format!(
                    "{} [SEP] {}",
                    doc.title_de
                        .as_deref()
                        .or(doc.title_en.as_deref())
                        .unwrap_or(""),
                    doc.description_de
                        .as_deref()
                        .or(doc.description_en.as_deref())
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
                    .map(|(mut doc, embedding)| {
                        doc.embedding = Some(embedding);
                        doc
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
        pipe::{embed::EmbeddingEnrichmentPipeImpl, spec::EnrichmentPipe},
    };
    use item_opensearch::item_document::ItemDocument;

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
            .enrich(fake::vec![ItemDocument; 42])
            .unwrap()
            .into_iter()
            .filter_map(|doc| doc.embedding)
            .collect::<Vec<_>>();

        assert_eq!(expected, actual);
    }
}
