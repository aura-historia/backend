use common::batch::Batch;
use common::language::domain::Language;
use pyo3::types::PyList;
use pyo3::{intern, prelude::*};
use pyo3_ffi::c_str;
use std::sync::Arc;

#[mockall::automock]
pub trait TranslationAdapter {
    fn translate_batch(
        &self,
        batch: &Batch<String, 64>,
        src: &Language,
        tgt: &Language,
    ) -> PyResult<Batch<String, 64>>;
}

#[derive(Clone)]
pub struct TranslationAdapterImpl {
    module: Arc<Py<PyAny>>,
}

impl TranslationAdapterImpl {
    pub fn new() -> PyResult<Self> {
        Python::attach(|py| -> PyResult<_> {
            let py_embed = c_str!(include_str!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/python/translate.py"
            )));
            let translate_module =
                PyModule::from_code(py, py_embed, c_str!("translate.py"), c_str!("translate"))?;
            Ok(TranslationAdapterImpl {
                module: Arc::new(translate_module.into()),
            })
        })
    }
}

impl TranslationAdapter for TranslationAdapterImpl {
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
                .getattr(py, intern!(py, "translate"))?
                .call1(
                    py,
                    (
                        py_texts,
                        src.format_human_readable(),
                        tgt.format_human_readable(),
                    ),
                )?;

            let translations: Vec<String> = result.as_ref().extract(py)?;
            let translations_batch = Batch::try_from(translations)
                .expect("shouldn't fail re-collecting former batch of same size");
            Ok(translations_batch)
        })
    }
}

#[cfg(any())]
#[cfg(test)]
mod tests {
    use crate::adapter::{TranslationAdapter, TranslationAdapterImpl};
    use common::language::domain::Language;

    #[test]
    #[ignore]
    fn should_translate_en_de() {
        let delegate = TranslationAdapterImpl::new().unwrap();

        let translation = delegate
            .translate("Hello world!", &Language::En, &Language::De)
            .unwrap();

        assert_eq!("Hallo Welt!", translation);
    }

    #[test]
    #[ignore]
    fn should_translate_batch_de_en() {
        let delegate = TranslationAdapterImpl::new().unwrap();

        let batch = vec!["Hallo Welt!".to_string(), "Wie geht es dir?".to_string()]
            .try_into()
            .unwrap();
        let translations = delegate
            .translate_batch(&batch, &Language::De, &Language::En)
            .unwrap();

        let expected = vec!["Hello world!".to_string(), "How are you?".to_string()];
        assert_eq!(expected, Vec::from(translations));
    }

    #[test]
    #[ignore]
    fn should_translate_batch_es_fr() {
        let delegate = TranslationAdapterImpl::new().unwrap();

        let batch = vec![
            "Este original reloj de pared es un modelo muy especial por su curioso diseño, muy actual y sorprendente en una pieza tan antigua. Es un reloj fabricado en Norteamérica en los años 20-30 del siglo XX, con caja de madera maciza y preciosos detalles decorativos en taracea. El reloj está muy bien conservado y se ha restaurado para mostrarse en todo su esplendor. La maquinaria también ha sido puesta a punto para garantizar un funcionamiento perfecto, de manera que el reloj da las horas y las medias con total precisión. La esfera es una pieza de cartón que sustituye a la original, actualmente desaparecida. Este detalle, sin embargo, no interfiere con la belleza del diseño o el magnífico funcionamiento del reloj. La caja es de un atractivo muy singular. Su parte superior, correspondiente a la esfera, tiene forma circular y no lleva adornos tallados ni torneados. Es un diseño muy limpio que adelanta las líneas a seguir por las artes decorativas modernas. La base es de madera de caoba maciza, adornada por un marco frontal compuesto por ocho piezas idénticas de madera de nogal con un filete de taracea en el centro. Los filetes de taracea se unen formando un delicado hexágono. Este adorno se repite en la parte inferior del reloj, tanto en la puerta del péndulo como en el detalle decorativo inferior. La puerta del péndulo es más pequeña y de forma rectangular, formando un conjunto muy equilibrado con el gran círculo superior. Lleva panel de cristal que deja ver el bonito péndulo, de latón grabado, y cuenta con un pomo en su parte inferior que sirve para abrirla. A los lados podemos ver sendas tallas simétricas realizadas en la caja de madera de caoba, que embellecen y equilibran el conjunto. En su parte inferior, el reloj está rematado por una pieza de caoba adornada con los mismos filetes de taracea y por una moldura que la separa visualmente de la puerta de acceso al péndulo. Este reloj es una pieza muy original. Su estilo contemporáneo lo convierte en el detalle perfecto para una casa con clase. Medidas: Ancho: 43 cm. Alto: 69 cm.".to_owned(),
            "Atractivo Reloj de Pared de Madera Maciza y Taracea. Norteamérica, 1920-30".to_owned(),
            "Antiguo Cristo de marfil con apliques de plata y certificado. Francia, siglo XVIII".to_owned(),
            "Magnífica y conmovedora representación de Cristo crucificado expirante, tallada en marfil de elefante y montada sobre una elegante cruz de madera ebonizada enriquecida con finos apliques de plata repujada. La obra procede de un taller francés y está datada en la segunda mitad del siglo XVIII, época en la que los mejores artífices del arte sacro europeo producían piezas de este tipo para capillas privadas y oratorios nobles. La escultura destaca por su gran calidad de talla y su fuerza expresiva.".to_owned()
        ]
        .try_into()
        .unwrap();
        let translations = delegate
            .translate_batch(&batch, &Language::Es, &Language::Fr)
            .unwrap();

        assert_eq!(4, translations.len());
    }
}
