use pyo3::types::PyList;
use pyo3::{intern, prelude::*};
use pyo3_ffi::c_str;
use std::sync::Arc;

// #[mockall::automock]
pub trait EmbeddingDelegate {
    /// Get embeddings for a batch of texts
    fn get_embeddings(&self, texts: &[&str]) -> PyResult<Vec<Vec<f32>>>;
}

/// Delegates to a persistent Python embedding session.
#[derive(Clone)]
pub struct EmbeddingDelegateImpl {
    module: Arc<Py<PyAny>>, // Keep module alive across GIL scopes
}

impl EmbeddingDelegateImpl {
    /// Initialize the session once
    pub fn new() -> PyResult<Self> {
        Python::attach(|py| -> PyResult<_> {
            let py_embed = c_str!(include_str!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/python/embed.py"
            )));
            let embed_module =
                PyModule::from_code(py, py_embed, c_str!("embed.py"), c_str!("embed"))?;
            Ok(EmbeddingDelegateImpl {
                module: Arc::new(embed_module.into()),
            })
        })
    }
}

impl EmbeddingDelegate for EmbeddingDelegateImpl {
    fn get_embeddings(&self, texts: &[&str]) -> PyResult<Vec<Vec<f32>>> {
        Python::attach(|py| -> PyResult<_> {
            let embed_module = self.module.as_ref();
            let py_texts = PyList::new(py, texts)?;

            // Call the Python function and convert to Py<PyAny> for safe ownership
            let result: Py<PyAny> = embed_module
                .getattr(py, intern!(py, "get_embeddings"))?
                .call1(py, (py_texts,))?;

            let embeddings: Vec<Vec<f32>> = result.as_ref().extract(py)?;
            Ok(embeddings)
        })
    }
}
