use common::batch::Batch;
use pyo3::types::PyList;
use pyo3::{intern, prelude::*};
use pyo3_ffi::c_str;
use std::sync::Arc;

#[mockall::automock]
pub trait ClassifyAdapter {
    fn classify(
        &self,
        batch: &Batch<(String, Vec<String>, Vec<String>), 64>,
    ) -> PyResult<Batch<(String, String), 64>>;
}

#[derive(Clone)]
pub struct ClassifyAdapterImpl {
    module: Arc<Py<PyAny>>, // Keep module alive across GIL scopes
}

impl ClassifyAdapterImpl {
    /// Initialize the session once
    pub fn new() -> PyResult<Self> {
        Python::attach(|py| -> PyResult<_> {
            let py_classify = c_str!(include_str!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/python/classify.py"
            )));
            let classify_module = PyModule::from_code(
                py,
                py_classify,
                c_str!("classify.py"),
                c_str!("classify"),
            )?;
            Ok(ClassifyAdapterImpl {
                module: Arc::new(classify_module.into()),
            })
        })
    }
}

impl ClassifyAdapter for ClassifyAdapterImpl {
    fn classify(
        &self,
        batch: &Batch<(String, Vec<String>, Vec<String>), 64>,
    ) -> PyResult<Batch<(String, String), 64>> {
        Python::attach(|py| -> PyResult<_> {
            let classify_module = self.module.as_ref();
            let py_batch = PyList::new(py, batch.iter())?;

            // Call the Python function and convert to Py<PyAny> for safe ownership
            let result: Py<PyAny> = classify_module
                .getattr(py, intern!(py, "classify"))?
                .call1(py, (py_batch,))?;

            let classifications: Vec<(String, String)> = result.as_ref().extract(py)?;
            let classifications_batch = Batch::try_from(classifications)
                .expect("shouldn't fail re-collecting former batch of same size");

            Ok(classifications_batch)
        })
    }
}
