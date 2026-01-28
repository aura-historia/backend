use common::batch::Batch;
use pyo3::types::PyList;
use pyo3::{intern, prelude::*};
use pyo3_ffi::c_str;
use std::sync::Arc;

#[async_trait::async_trait]
#[mockall::automock]
pub trait ExtractionAdapter {
    fn extract(&self, schema: &str, batch: &Batch<String, 8>) -> PyResult<Batch<String, 8>>;
}

#[derive(Clone)]
pub struct ExtractionAdapterImpl {
    module: Arc<Py<PyAny>>, // Keep module alive across GIL scopes
}

impl ExtractionAdapterImpl {
    /// Initialize the session once
    pub fn new() -> PyResult<Self> {
        Python::attach(|py| -> PyResult<_> {
            let py_extract = c_str!(include_str!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/python/extract.py"
            )));
            let extract_module =
                PyModule::from_code(py, py_extract, c_str!("extract.py"), c_str!("extract"))?;
            Ok(ExtractionAdapterImpl {
                module: Arc::new(extract_module.into()),
            })
        })
    }
}

impl ExtractionAdapter for ExtractionAdapterImpl {
    fn extract(&self, schema: &str, batch: &Batch<String, 8>) -> PyResult<Batch<String, 8>> {
        Python::attach(|py| -> PyResult<_> {
            let extract_module = self.module.as_ref();
            let py_texts = PyList::new(py, batch.iter())?;

            // Call the Python function and convert to Py<PyAny> for safe ownership
            let result: Py<PyAny> = extract_module
                .getattr(py, intern!(py, "extract"))?
                .call1(py, (schema, py_texts))?;

            let extractions: Vec<String> = result.as_ref().extract(py)?;
            let extractions_batch = Batch::try_from(extractions)
                .expect("shouldn't fail re-collecting former batch of same size");

            Ok(extractions_batch)
        })
    }
}
