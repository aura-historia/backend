use common::batch::Batch;
use pyo3::types::PyList;
use pyo3::{intern, prelude::*};
use pyo3_ffi::c_str;
use std::sync::Arc;

#[mockall::automock]
pub trait ClassifyCategoryAdapter {
    fn classify_category(
        &self,
        batch: &Batch<(String, Vec<String>), 64>,
    ) -> PyResult<Batch<String, 64>>;
}

#[derive(Clone)]
pub struct ClassifyCategoryAdapterImpl {
    module: Arc<Py<PyAny>>, // Keep module alive across GIL scopes
}

impl ClassifyCategoryAdapterImpl {
    /// Initialize the session once
    pub fn new() -> PyResult<Self> {
        Python::attach(|py| -> PyResult<_> {
            let py_classify_category = c_str!(include_str!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/python/classify_category.py"
            )));
            let classify_category_module = PyModule::from_code(
                py,
                py_classify_category,
                c_str!("classify_category.py"),
                c_str!("classify_category"),
            )?;
            Ok(ClassifyCategoryAdapterImpl {
                module: Arc::new(classify_category_module.into()),
            })
        })
    }
}

impl ClassifyCategoryAdapter for ClassifyCategoryAdapterImpl {
    fn classify_category(
        &self,
        batch: &Batch<(String, Vec<String>), 64>,
    ) -> PyResult<Batch<String, 64>> {
        Python::attach(|py| -> PyResult<_> {
            let classify_category_module = self.module.as_ref();
            let py_batch = PyList::new(py, batch.iter())?;

            // Call the Python function and convert to Py<PyAny> for safe ownership
            let result: Py<PyAny> = classify_category_module
                .getattr(py, intern!(py, "classify_category"))?
                .call1(py, (py_batch,))?;

            let classifications: Vec<String> = result.as_ref().extract(py)?;
            let classifications_batch = Batch::try_from(classifications)
                .expect("shouldn't fail re-collecting former batch of same size");

            Ok(classifications_batch)
        })
    }
}
