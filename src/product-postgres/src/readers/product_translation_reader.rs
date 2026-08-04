use common::language::domain::Language;
use common::product_id::ProductId;
use product_core::description::Description;
use product_core::title::Title;
use product_service::ports::{
    ProductTranslationReadError, ProductTranslationReader, ProductTranslationReaderFactory,
    ProductTranslationsView,
};
use sqlx::PgConnection;
use std::collections::{HashMap, HashSet};

#[derive(Debug, Clone, Copy, Default)]
pub struct SqlxProductTranslationReaderFactory;

struct SqlxProductTranslationReader<'tx> {
    connection: &'tx mut PgConnection,
}

#[derive(Debug, sqlx::FromRow)]
struct ProductTranslationRow {
    product_id: uuid::Uuid,
    language: String,
    title: Option<String>,
    description: Option<String>,
}

impl SqlxProductTranslationReaderFactory {
    pub fn new() -> Self {
        Self
    }
}

impl ProductTranslationReaderFactory<common::postgres::SqlxTransaction>
    for SqlxProductTranslationReaderFactory
{
    fn in_transaction<'tx>(
        &'tx self,
        tx: &'tx mut common::postgres::SqlxTransaction,
    ) -> impl ProductTranslationReader + 'tx {
        SqlxProductTranslationReader {
            connection: tx.connection(),
        }
    }
}

#[async_trait::async_trait]
impl ProductTranslationReader for SqlxProductTranslationReader<'_> {
    async fn find_for_product(
        &mut self,
        product_id: ProductId,
    ) -> Result<ProductTranslationsView, ProductTranslationReadError> {
        let rows = sqlx::query_as::<_, ProductTranslationRow>(
            r#"
            SELECT product_id, language, title, description
            FROM product_translations
            WHERE product_id = $1
            "#,
        )
        .bind(uuid::Uuid::from(product_id))
        .fetch_all(&mut *self.connection)
        .await
        .map_err(|_| ProductTranslationReadError::ProductTranslationLookupFailed)?;

        translations_view(product_id, rows)
    }
}

fn translations_view(
    product_id: ProductId,
    rows: Vec<ProductTranslationRow>,
) -> Result<ProductTranslationsView, ProductTranslationReadError> {
    let mut languages = HashSet::new();
    let mut titles = HashMap::new();
    let mut descriptions = HashMap::new();

    for row in rows {
        if ProductId::from(row.product_id) != product_id {
            return Err(ProductTranslationReadError::ProductTranslationReadModelInvalid);
        }

        let language = language(&row.language)?;
        if !languages.insert(language) {
            return Err(ProductTranslationReadError::ProductTranslationReadModelInvalid);
        }

        match (row.title, row.description) {
            (None, None) => {
                return Err(ProductTranslationReadError::ProductTranslationReadModelInvalid);
            }
            (title, description) => {
                if let Some(title_text) = title {
                    titles.insert(language, translation_title(&title_text)?);
                }
                if let Some(description_text) = description {
                    descriptions.insert(language, translation_description(&description_text)?);
                }
            }
        }
    }

    Ok(ProductTranslationsView {
        product_id,
        titles,
        descriptions,
    })
}

fn language(value: &str) -> Result<Language, ProductTranslationReadError> {
    match value {
        "de" => Ok(Language::De),
        "en" => Ok(Language::En),
        "fr" => Ok(Language::Fr),
        "es" => Ok(Language::Es),
        "it" => Ok(Language::It),
        "zh" => Ok(Language::Zh),
        "pt" => Ok(Language::Pt),
        "pl" => Ok(Language::Pl),
        "tr" => Ok(Language::Tr),
        "nl" => Ok(Language::Nl),
        "cs" => Ok(Language::Cs),
        "ja" => Ok(Language::Ja),
        "ru" => Ok(Language::Ru),
        "ar" => Ok(Language::Ar),
        _ => Err(ProductTranslationReadError::ProductTranslationReadModelInvalid),
    }
}

fn translation_title(value: &str) -> Result<Title, ProductTranslationReadError> {
    let title = Title::from(value);
    if title.as_ref().is_empty() || title.as_ref() != value {
        return Err(ProductTranslationReadError::ProductTranslationReadModelInvalid);
    }
    Ok(title)
}

fn translation_description(value: &str) -> Result<Description, ProductTranslationReadError> {
    let description = Description::from(value);
    if description.as_ref().is_empty() || description.as_ref() != value {
        return Err(ProductTranslationReadError::ProductTranslationReadModelInvalid);
    }
    Ok(description)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(
        product_id: uuid::Uuid,
        language: &str,
        title: Option<&str>,
        description: Option<&str>,
    ) -> ProductTranslationRow {
        ProductTranslationRow {
            product_id,
            language: language.to_owned(),
            title: title.map(str::to_owned),
            description: description.map(str::to_owned),
        }
    }

    #[test]
    fn should_map_translation_rows() -> Result<(), ProductTranslationReadError> {
        let product_id = ProductId::from(uuid::Uuid::nil());

        let view = translations_view(
            product_id,
            vec![
                row(uuid::Uuid::nil(), "en", Some("Translated title"), None),
                row(
                    uuid::Uuid::nil(),
                    "de",
                    None,
                    Some("Übersetzte Beschreibung"),
                ),
            ],
        )?;

        assert_eq!(view.product_id, product_id);
        assert_eq!(view.titles[&Language::En].as_ref(), "Translated title");
        assert_eq!(
            view.descriptions[&Language::De].as_ref(),
            "Übersetzte Beschreibung"
        );
        Ok(())
    }

    #[test]
    fn should_reject_translation_row_when_language_is_unrecognized() {
        let result = translations_view(
            ProductId::from(uuid::Uuid::nil()),
            vec![row(uuid::Uuid::nil(), "xx", Some("Title"), None)],
        );

        assert!(matches!(
            result,
            Err(ProductTranslationReadError::ProductTranslationReadModelInvalid)
        ));
    }

    #[test]
    fn should_reject_translation_row_when_content_is_missing() {
        let result = translations_view(
            ProductId::from(uuid::Uuid::nil()),
            vec![row(uuid::Uuid::nil(), "en", None, None)],
        );

        assert!(matches!(
            result,
            Err(ProductTranslationReadError::ProductTranslationReadModelInvalid)
        ));
    }

    #[test]
    fn should_reject_translation_row_when_title_is_not_canonical() {
        let result = translations_view(
            ProductId::from(uuid::Uuid::nil()),
            vec![row(uuid::Uuid::nil(), "en", Some("title"), None)],
        );

        assert!(matches!(
            result,
            Err(ProductTranslationReadError::ProductTranslationReadModelInvalid)
        ));
    }

    #[test]
    fn should_reject_translation_row_when_description_is_not_canonical() {
        let result = translations_view(
            ProductId::from(uuid::Uuid::nil()),
            vec![row(uuid::Uuid::nil(), "en", None, Some(" "))],
        );

        assert!(matches!(
            result,
            Err(ProductTranslationReadError::ProductTranslationReadModelInvalid)
        ));
    }

    #[test]
    fn should_reject_translation_rows_when_product_id_does_not_match() {
        let result = translations_view(
            ProductId::from(uuid::Uuid::nil()),
            vec![row(uuid::Uuid::from_u128(1), "en", Some("Title"), None)],
        );

        assert!(matches!(
            result,
            Err(ProductTranslationReadError::ProductTranslationReadModelInvalid)
        ));
    }

    #[test]
    fn should_reject_translation_rows_when_language_is_repeated() {
        let result = translations_view(
            ProductId::from(uuid::Uuid::nil()),
            vec![
                row(uuid::Uuid::nil(), "en", Some("Title"), None),
                row(uuid::Uuid::nil(), "en", None, Some("Description")),
            ],
        );

        assert!(matches!(
            result,
            Err(ProductTranslationReadError::ProductTranslationReadModelInvalid)
        ));
    }
}
