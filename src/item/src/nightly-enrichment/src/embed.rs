use common::batch::Batch;
use pyo3::types::PyList;
use pyo3::{intern, prelude::*};
use pyo3_ffi::c_str;
use std::sync::Arc;

#[mockall::automock]
pub trait EmbeddingDelegate {
    fn embed(&self, batch: &Batch<String, 64>) -> PyResult<Batch<Vec<f32>, 64>>;
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
    fn embed(&self, batch: &Batch<String, 64>) -> PyResult<Batch<Vec<f32>, 64>> {
        Python::attach(|py| -> PyResult<_> {
            let embed_module = self.module.as_ref();
            let py_texts = PyList::new(py, batch.iter())?;

            // Call the Python function and convert to Py<PyAny> for safe ownership
            let result: Py<PyAny> = embed_module
                .getattr(py, intern!(py, "embed"))?
                .call1(py, (py_texts,))?;

            let embeddings: Vec<Vec<f32>> = result.as_ref().extract(py)?;
            let embeddings_batch = Batch::try_from(embeddings)
                .expect("shouldn't fail re-collecting former batch of same size");
            Ok(embeddings_batch)
        })
    }
}

#[cfg(test)]
mod tests {
    use crate::embed::{EmbeddingDelegate, EmbeddingDelegateImpl};

    #[test]
    #[ignore]
    fn should_embed_dim_1024() {
        let delegate = EmbeddingDelegateImpl::new().unwrap();

        let batch = vec![
            "foo".to_owned(),
            "bar".to_owned(),
            "baaaaaaaaaaaaaaaz [SEP] bat".to_owned(),
            "baaaaaaaaaaaaaaaz".to_owned(),
        ]
        .try_into()
        .unwrap();
        let embeddings = delegate.embed(&batch).unwrap();

        assert_eq!(batch.len(), embeddings.len());
        assert!(embeddings.iter().all(|embedding| embedding.len() == 1024));
    }
}
