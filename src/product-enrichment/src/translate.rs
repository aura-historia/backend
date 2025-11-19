use common::batch::Batch;
use common::language::domain::Language;
use pyo3::types::PyList;
use pyo3::{intern, prelude::*};
use pyo3_ffi::c_str;
use std::sync::Arc;

#[mockall::automock]
pub trait TranslationDelegate {
    fn translate(&self, text: &str, src: &Language, tgt: &Language) -> PyResult<String>;
    fn translate_batch(
        &self,
        batch: &Batch<String, 64>,
        src: &Language,
        tgt: &Language,
    ) -> PyResult<Batch<String, 64>>;
}

#[derive(Clone)]
pub struct TranslationDelegateImpl {
    module: Arc<Py<PyAny>>,
}

impl TranslationDelegateImpl {
    pub fn new() -> PyResult<Self> {
        Python::attach(|py| -> PyResult<_> {
            let py_embed = c_str!(include_str!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/python/translate.py"
            )));
            let translate_module =
                PyModule::from_code(py, py_embed, c_str!("translate.py"), c_str!("translate"))?;
            Ok(TranslationDelegateImpl {
                module: Arc::new(translate_module.into()),
            })
        })
    }
}

impl TranslationDelegate for TranslationDelegateImpl {
    fn translate(&self, text: &str, src: &Language, tgt: &Language) -> PyResult<String> {
        Python::attach(|py| -> PyResult<_> {
            let translate_module = self.module.as_ref();

            // Call the Python function and convert to Py<PyAny> for safe ownership
            let result: Py<PyAny> = translate_module
                .getattr(py, intern!(py, "translate"))?
                .call1(py, (text, src.as_str(), tgt.as_str()))?;

            result.as_ref().extract(py)
        })
    }

    fn translate_batch(
        &self,
        batch: &Batch<String, 64>,
        src: &Language,
        tgt: &Language,
    ) -> PyResult<Batch<String, 64>> {
        Python::attach(|py| -> PyResult<_> {
            let translate_module = self.module.as_ref();
            let py_texts = PyList::new(py, batch.iter())?;

            // Call the Python function and convert to Py<PyAny> for safe ownership
            let result: Py<PyAny> = translate_module
                .getattr(py, intern!(py, "translate_batch"))?
                .call1(py, (py_texts, src.as_str(), tgt.as_str()))?;

            let translations: Vec<String> = result.as_ref().extract(py)?;
            let translations_batch = Batch::try_from(translations)
                .expect("shouldn't fail re-collecting former batch of same size");
            Ok(translations_batch)
        })
    }
}

#[cfg(test)]
mod tests {
    use crate::translate::{TranslationDelegate, TranslationDelegateImpl};
    use common::language::domain::Language;

    #[test]
    #[ignore]
    fn should_translate_en_de() {
        let delegate = TranslationDelegateImpl::new().unwrap();

        let translation = delegate
            .translate("Hello world!", &Language::En, &Language::De)
            .unwrap();

        assert_eq!("Hallo Welt!", translation);
    }

    #[test]
    #[ignore]
    fn should_translate_batch_de_en() {
        let delegate = TranslationDelegateImpl::new().unwrap();

        let batch = vec!["Hallo Welt!".to_string(), "Wie geht es dir?".to_string()]
            .try_into()
            .unwrap();
        let translations = delegate
            .translate_batch(&batch, &Language::De, &Language::En)
            .unwrap();

        let expected = vec!["Hello world!".to_string(), "How are you?".to_string()];
        assert_eq!(expected, Vec::from(translations));
    }
}
