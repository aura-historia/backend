use nightly_enrichment::embed::{EmbeddingDelegate, EmbeddingDelegateImpl};
use pyo3::PyResult;

fn main() -> PyResult<()> {
    let session = EmbeddingDelegateImpl::new()?;

    let batch1 = [
        "Antique Wehrmacht cap 1941".to_owned(),
        "Vintage German helmet".to_owned(),
    ];
    let batch2 = ["19th century vase".to_owned(), "Ancient coin".to_owned()];

    let embeddings1 = session.embed(&batch1.into())?;
    let embeddings2 = session.embed(&batch2.into())?;

    println!("Batch1 embeddings: {}", embeddings1.len());
    println!("Batch2 embeddings: {}", embeddings2.len());
    println!("Dim of first embedding: {}", embeddings1[0].len());

    Ok(())
}
