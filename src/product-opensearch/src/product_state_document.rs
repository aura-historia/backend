use product_core::product_state::ProductState;
use serde::{Deserialize, Serialize};

#[derive(
    Copy, Clone, Eq, PartialEq, Debug, Hash, Serialize, Deserialize, strum_macros::EnumCount,
)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub(crate) enum ProductStateDocument {
    Listed,
    Available,
    Reserved,
    Sold,
    Removed,
    Unknown,
}

impl From<ProductState> for ProductStateDocument {
    fn from(value: ProductState) -> Self {
        match value {
            ProductState::Listed => Self::Listed,
            ProductState::Available => Self::Available,
            ProductState::Reserved => Self::Reserved,
            ProductState::Sold => Self::Sold,
            ProductState::Removed => Self::Removed,
            ProductState::Unknown => Self::Unknown,
        }
    }
}

impl From<ProductStateDocument> for ProductState {
    fn from(value: ProductStateDocument) -> Self {
        match value {
            ProductStateDocument::Listed => Self::Listed,
            ProductStateDocument::Available => Self::Available,
            ProductStateDocument::Reserved => Self::Reserved,
            ProductStateDocument::Sold => Self::Sold,
            ProductStateDocument::Removed => Self::Removed,
            ProductStateDocument::Unknown => Self::Unknown,
        }
    }
}

impl ProductStateDocument {
    pub(crate) fn as_str(&self) -> &'static str {
        match self {
            Self::Listed => "LISTED",
            Self::Available => "AVAILABLE",
            Self::Reserved => "RESERVED",
            Self::Sold => "SOLD",
            Self::Removed => "REMOVED",
            Self::Unknown => "UNKNOWN",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[rstest::rstest]
    #[case(ProductStateDocument::Listed, "\"LISTED\"")]
    #[case(ProductStateDocument::Available, "\"AVAILABLE\"")]
    #[case(ProductStateDocument::Reserved, "\"RESERVED\"")]
    #[case(ProductStateDocument::Sold, "\"SOLD\"")]
    #[case(ProductStateDocument::Removed, "\"REMOVED\"")]
    #[case(ProductStateDocument::Unknown, "\"UNKNOWN\"")]
    fn should_serialize_product_state_document_in_screaming_snake_case(
        #[case] state: ProductStateDocument,
        #[case] expected: &'static str,
    ) -> Result<(), serde_json::Error> {
        assert_eq!(expected, serde_json::to_string(&state)?);
        assert_eq!(expected.replace('"', ""), state.as_str());
        Ok(())
    }

    #[rstest::rstest]
    #[case(ProductState::Listed, ProductStateDocument::Listed)]
    #[case(ProductState::Available, ProductStateDocument::Available)]
    #[case(ProductState::Reserved, ProductStateDocument::Reserved)]
    #[case(ProductState::Sold, ProductStateDocument::Sold)]
    #[case(ProductState::Removed, ProductStateDocument::Removed)]
    #[case(ProductState::Unknown, ProductStateDocument::Unknown)]
    fn should_roundtrip_product_state(
        #[case] domain: ProductState,
        #[case] document: ProductStateDocument,
    ) {
        assert_eq!(document, ProductStateDocument::from(domain));
        assert_eq!(domain, ProductState::from(document));
    }
}
