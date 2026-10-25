//! The non-secret Shopify replacement body used by REST.
use crate::product_listing_management::MutationInputError;
use listing_source_core::Domain;
use listing_source_service::ports::ListingIngestionConfiguration;
use schemars::JsonSchema;
use serde::Deserialize;

#[derive(Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PutShopifyConfigurationData {
    pub domain: String,
    pub currency: Option<String>,
    pub language: Option<String>,
}
impl PutShopifyConfigurationData {
    pub fn into_configuration(self) -> Result<ListingIngestionConfiguration, MutationInputError> {
        let domain = Domain::try_from(self.domain).map_err(|error| {
            MutationInputError::caused("domain", "Body field 'domain' is invalid.", error)
        })?;
        let currency = self
            .currency
            .map(|value| {
                money::Currency::from_code(&value).ok_or_else(|| {
                    MutationInputError::invalid("currency", "Body field 'currency' is invalid.")
                })
            })
            .transpose()?;
        let language = self
            .language
            .map(|value| {
                localization::Language::from_code(&value).ok_or_else(|| {
                    MutationInputError::invalid("language", "Body field 'language' is invalid.")
                })
            })
            .transpose()?;
        Ok(ListingIngestionConfiguration::Shopify {
            domain,
            currency,
            language,
        })
    }
}
