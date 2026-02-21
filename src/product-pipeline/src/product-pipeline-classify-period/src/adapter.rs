use common::batch::Batch;
use pyo3::types::PyList;
use pyo3::{intern, prelude::*};
use pyo3_ffi::c_str;
use std::sync::Arc;

#[mockall::automock]
pub trait ClassifyPeriodAdapter {
    fn classify_period(
        &self,
        batch: &Batch<(String, Vec<String>), 64>,
    ) -> PyResult<Batch<String, 64>>;
}

#[derive(Clone)]
pub struct ClassifyPeriodAdapterImpl {
    module: Arc<Py<PyAny>>, // Keep module alive across GIL scopes
}

impl ClassifyPeriodAdapterImpl {
    /// Initialize the session once
    pub fn new() -> PyResult<Self> {
        Python::attach(|py| -> PyResult<_> {
            let py_classify_period = c_str!(include_str!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/python/classify_period.py"
            )));
            let classify_period_module = PyModule::from_code(
                py,
                py_classify_period,
                c_str!("classify_period.py"),
                c_str!("classify_period"),
            )?;
            Ok(ClassifyPeriodAdapterImpl {
                module: Arc::new(classify_period_module.into()),
            })
        })
    }
}

impl ClassifyPeriodAdapter for ClassifyPeriodAdapterImpl {
    fn classify_period(
        &self,
        batch: &Batch<(String, Vec<String>), 64>,
    ) -> PyResult<Batch<String, 64>> {
        Python::attach(|py| -> PyResult<_> {
            let classify_period_module = self.module.as_ref();
            let py_batch = PyList::new(py, batch.iter())?;

            // Call the Python function and convert to Py<PyAny> for safe ownership
            let result: Py<PyAny> = classify_period_module
                .getattr(py, intern!(py, "classify_period"))?
                .call1(py, (py_batch,))?;

            let classifications: Vec<String> = result.as_ref().extract(py)?;
            let classifications_batch = Batch::try_from(classifications)
                .expect("shouldn't fail re-collecting former batch of same size");

            Ok(classifications_batch)
        })
    }
}
