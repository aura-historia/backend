use pyo3::prelude::*;
use pyo3::types::PyList;
use pyo3_ffi::c_str;
use std::sync::Arc;

fn main() -> PyResult<()> {
    let session = EmbeddingSession::new()?;

    let batch1 = vec!["Antique Wehrmacht cap 1941", "Vintage German helmet"];
    let batch2 = vec!["19th century vase", "Ancient coin"];

    let embeddings1 = session.get_embeddings(&batch1)?;
    let embeddings2 = session.get_embeddings(&batch2)?;

    println!("Batch1 embeddings: {}", embeddings1.len());
    println!("Batch2 embeddings: {}", embeddings2.len());
    println!("Dim of first embedding: {}", embeddings1[0].len());

    Ok(())
}

/// A persistent Python embedding session
#[derive(Clone)]
struct EmbeddingSession {
    module: Arc<Py<PyAny>>, // Keep module alive across GIL scopes
}

impl EmbeddingSession {
    /// Initialize the session once
    fn new() -> PyResult<Self> {
        Python::attach(|py| -> PyResult<_> {
            let py_embed = c_str!(include_str!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/python/embed.py"
            )));
            let embed_module =
                PyModule::from_code(py, py_embed, c_str!("embed.py"), c_str!("embed"))?;
            Ok(EmbeddingSession {
                module: Arc::new(embed_module.into()),
            })
        })
    }

    /// Get embeddings for a batch of texts
    fn get_embeddings(&self, texts: &[&str]) -> PyResult<Vec<Vec<f32>>> {
        Python::attach(|py| -> PyResult<_> {
            let embed_module = self.module.as_ref();
            let py_texts = PyList::new(py, texts)?;

            // Call the Python function and convert to Py<PyAny> for safe ownership
            let result: Py<PyAny> = embed_module
                .getattr(py, "get_embeddings")?
                .call1(py, (py_texts,))?;

            let embeddings: Vec<Vec<f32>> = result.as_ref().extract(py)?;
            Ok(embeddings)
        })
    }
}
