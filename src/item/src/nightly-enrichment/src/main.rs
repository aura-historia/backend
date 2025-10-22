use nightly_enrichment::embed::{EmbeddingDelegate, EmbeddingDelegateImpl};
use pyo3::PyResult;

fn main() -> PyResult<()> {
    let session = EmbeddingDelegateImpl::new()?;

    let batch1 = vec!["Antique Wehrmacht cap 1941", "Vintage German helmet"];
    let batch2 = vec!["19th century vase", "Ancient coin"];

    let embeddings1 = session.get_embeddings(&batch1)?;
    let embeddings2 = session.get_embeddings(&batch2)?;

    println!("Batch1 embeddings: {}", embeddings1.len());
    println!("Batch2 embeddings: {}", embeddings2.len());
    println!("Dim of first embedding: {}", embeddings1[0].len());

    Ok(())
}
