use product_core::product_lifecycle::ProductLifecycle;
use serde::{Deserialize, Serialize};

#[derive(Copy, Clone, Eq, PartialEq, Debug, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub(crate) enum ProductLifecycleDocument {
    #[default]
    Active,
    Deleted,
}

impl From<ProductLifecycle> for ProductLifecycleDocument {
    fn from(value: ProductLifecycle) -> Self {
        match value {
            ProductLifecycle::Active => Self::Active,
            ProductLifecycle::Deleted => Self::Deleted,
        }
    }
}

impl From<ProductLifecycleDocument> for ProductLifecycle {
    fn from(value: ProductLifecycleDocument) -> Self {
        match value {
            ProductLifecycleDocument::Active => Self::Active,
            ProductLifecycleDocument::Deleted => Self::Deleted,
        }
    }
}

impl ProductLifecycleDocument {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Active => "ACTIVE",
            Self::Deleted => "DELETED",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[rstest::rstest]
    #[case(ProductLifecycleDocument::Active, "\"ACTIVE\"")]
    #[case(ProductLifecycleDocument::Deleted, "\"DELETED\"")]
    fn should_serialize_product_lifecycle_document_in_screaming_snake_case(
        #[case] lifecycle: ProductLifecycleDocument,
        #[case] expected: &'static str,
    ) -> Result<(), serde_json::Error> {
        assert_eq!(expected, serde_json::to_string(&lifecycle)?);
        assert_eq!(expected.replace('"', ""), lifecycle.as_str());
        Ok(())
    }
}
