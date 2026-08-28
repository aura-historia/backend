use crate::error::{ApiError, BAD_BODY_VALUE};
use crate::patch_value::{PatchValue, clearable, non_nullable_patch};
use application::patch_field::PatchField;
use listing_source_core::{AcquisitionMethod, Domain, ListingSourceId, ReferralConfiguration};
use listing_source_service::ports::{
    AcquisitionConfiguration, ListingSourceAcquisitionConfigurations, ListingSourceDetails,
};
use partnership_service::ports::AdministeredListingSource;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::str::FromStr;
use url::Url;
use uuid::Uuid;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CreateListingSourceData {
    pub(crate) name: String,
    pub(crate) operator_party_id: Uuid,
    #[serde(deserialize_with = "deserialize_acquisition_methods")]
    pub(crate) acquisition_methods: HashSet<AcquisitionMethod>,
    pub(crate) acquisition_configuration: Vec<AcquisitionConfigurationData>,
    #[serde(default)]
    pub(crate) woocommerce_webhook_secret: Option<String>,
    #[serde(default)]
    pub(crate) url: Option<Url>,
    #[serde(default)]
    pub(crate) image: Option<Url>,
    #[serde(default)]
    pub(crate) referral_configuration: Option<ReferralConfigurationData>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct UpdateListingSourceData {
    #[serde(default)]
    pub(crate) name: PatchValue<String>,
    #[serde(default)]
    pub(crate) operator_party_id: PatchValue<Uuid>,
    #[serde(default, deserialize_with = "deserialize_patch_acquisition_methods")]
    pub(crate) acquisition_methods: PatchValue<HashSet<AcquisitionMethod>>,
    #[serde(default)]
    pub(crate) acquisition_configuration: PatchValue<Vec<AcquisitionConfigurationData>>,
    #[serde(default)]
    pub(crate) woocommerce_webhook_secret: PatchValue<String>,
    #[serde(default)]
    pub(crate) url: PatchValue<Url>,
    #[serde(default)]
    pub(crate) image: PatchValue<Url>,
    #[serde(default)]
    pub(crate) referral_configuration: PatchValue<ReferralConfigurationData>,
}

impl UpdateListingSourceData {
    pub(crate) fn into_parts(self) -> Result<UpdateListingSourceDataParts, ApiError> {
        Ok(UpdateListingSourceDataParts {
            name: map_patch(non_nullable_patch(self.name, "name")?, Into::into),
            operator_party_id: map_patch(
                non_nullable_patch(self.operator_party_id, "operatorPartyId")?,
                Into::into,
            ),
            acquisition_methods: non_nullable_patch(
                self.acquisition_methods,
                "acquisitionMethods",
            )?,
            acquisition_configuration: map_patch_result(
                non_nullable_patch(self.acquisition_configuration, "acquisitionConfiguration")?,
                configurations,
            )?,
            woocommerce_webhook_secret: clearable(self.woocommerce_webhook_secret),
            url: clearable(self.url),
            image: clearable(self.image),
            referral_configuration: clearable(self.referral_configuration.map(Into::into)),
        })
    }
}

pub(crate) struct UpdateListingSourceDataParts {
    pub(crate) name: PatchField<listing_source_core::ListingSourceName>,
    pub(crate) operator_party_id: PatchField<party_core::party_id::PartyId>,
    pub(crate) acquisition_methods: PatchField<HashSet<AcquisitionMethod>>,
    pub(crate) acquisition_configuration: PatchField<ListingSourceAcquisitionConfigurations>,
    pub(crate) woocommerce_webhook_secret: PatchField<String>,
    pub(crate) url: PatchField<Url>,
    pub(crate) image: PatchField<Url>,
    pub(crate) referral_configuration: PatchField<ReferralConfiguration>,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
pub(crate) enum AcquisitionConfigurationData {
    #[serde(rename = "UNCONFIGURED")]
    Unconfigured {
        #[serde(
            rename = "acquisitionMethod",
            deserialize_with = "deserialize_acquisition_method"
        )]
        acquisition_method: AcquisitionMethod,
    },
    #[serde(rename = "WEB_CRAWL")]
    WebCrawl,
    #[serde(rename = "SHOPIFY")]
    Shopify {
        domain: String,
        #[serde(default)]
        currency: Option<String>,
        #[serde(default)]
        language: Option<String>,
    },
    #[serde(rename = "WOOCOMMERCE")]
    Woocommerce {
        #[serde(default)]
        currency: Option<String>,
        #[serde(default)]
        language: Option<String>,
    },
    #[serde(rename = "PARTNER_API")]
    PartnerApi,
}

impl TryFrom<AcquisitionConfigurationData> for AcquisitionConfiguration {
    type Error = ApiError;

    fn try_from(value: AcquisitionConfigurationData) -> Result<Self, Self::Error> {
        match value {
            AcquisitionConfigurationData::Unconfigured { acquisition_method } => {
                Ok(Self::Unconfigured(acquisition_method))
            }
            AcquisitionConfigurationData::WebCrawl => Ok(Self::WebCrawl),
            AcquisitionConfigurationData::Shopify {
                domain,
                currency,
                language,
            } => Ok(Self::Shopify {
                domain: Domain::try_from(domain).map_err(|_| invalid_body("domain"))?,
                currency: parse_currency(currency)?,
                language: parse_language(language)?,
            }),
            AcquisitionConfigurationData::Woocommerce { currency, language } => {
                Ok(Self::Woocommerce {
                    currency: parse_currency(currency)?,
                    language: parse_language(language)?,
                })
            }
            AcquisitionConfigurationData::PartnerApi => Ok(Self::PartnerApi),
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
pub(crate) enum ReferralConfigurationData {
    #[serde(rename = "PARTNERIZE")]
    Partnerize { camref: String },
}

impl From<ReferralConfigurationData> for ReferralConfiguration {
    fn from(value: ReferralConfigurationData) -> Self {
        match value {
            ReferralConfigurationData::Partnerize { camref } => Self::Partnerize { camref },
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ListingSourceData {
    listing_source_id: String,
    listing_source_slug_id: String,
    name: String,
    operator_party_id: String,
    operator_slug_id: String,
    operator_name: String,
    acquisition_methods: Vec<AcquisitionMethodData>,
    #[serde(skip_serializing_if = "Option::is_none")]
    url: Option<Url>,
    #[serde(skip_serializing_if = "Option::is_none")]
    image: Option<Url>,
    #[serde(with = "time::serde::rfc3339")]
    created: time::OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    updated: time::OffsetDateTime,
}

impl From<ListingSourceDetails> for ListingSourceData {
    fn from(value: ListingSourceDetails) -> Self {
        Self {
            listing_source_id: value.listing_source_id.to_string(),
            listing_source_slug_id: value.slug_id.to_string(),
            name: value.name.to_string(),
            operator_party_id: value.operator_party_id.to_string(),
            operator_slug_id: value.operator_slug_id.to_string(),
            operator_name: value.operator_name.to_string(),
            acquisition_methods: methods_data(value.acquisition_methods),
            url: value.url,
            image: value.image,
            created: value.created,
            updated: value.updated,
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ListingSourceReferenceData {
    listing_source_id: String,
    listing_source_slug_id: String,
}

impl From<(ListingSourceId, listing_source_core::ListingSourceSlugId)>
    for ListingSourceReferenceData
{
    fn from(
        (listing_source_id, slug_id): (ListingSourceId, listing_source_core::ListingSourceSlugId),
    ) -> Self {
        Self {
            listing_source_id: listing_source_id.to_string(),
            listing_source_slug_id: slug_id.to_string(),
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AdministeredListingSourceData {
    listing_source_id: String,
    listing_source_slug_id: String,
    name: String,
}

impl From<AdministeredListingSource> for AdministeredListingSourceData {
    fn from(value: AdministeredListingSource) -> Self {
        Self {
            listing_source_id: value.listing_source_id.to_string(),
            listing_source_slug_id: value.slug_id.to_string(),
            name: value.name.to_string(),
        }
    }
}

#[derive(Debug, Serialize)]
pub(crate) enum AcquisitionMethodData {
    #[serde(rename = "WEB_CRAWL")]
    WebCrawl,
    #[serde(rename = "SHOPIFY")]
    Shopify,
    #[serde(rename = "WOOCOMMERCE")]
    Woocommerce,
    #[serde(rename = "PARTNER_API")]
    PartnerApi,
}

impl From<AcquisitionMethod> for AcquisitionMethodData {
    fn from(value: AcquisitionMethod) -> Self {
        match value {
            AcquisitionMethod::WebCrawl => Self::WebCrawl,
            AcquisitionMethod::Shopify => Self::Shopify,
            AcquisitionMethod::Woocommerce => Self::Woocommerce,
            AcquisitionMethod::PartnerApi => Self::PartnerApi,
        }
    }
}

fn map_patch<T, U>(value: PatchField<T>, map: impl FnOnce(T) -> U) -> PatchField<U> {
    match value {
        PatchField::Unchanged => PatchField::Unchanged,
        PatchField::Set(value) => PatchField::Set(map(value)),
        PatchField::Clear => PatchField::Clear,
    }
}

fn map_patch_result<T, U>(
    value: PatchField<T>,
    map: impl FnOnce(T) -> Result<U, ApiError>,
) -> Result<PatchField<U>, ApiError> {
    match value {
        PatchField::Unchanged => Ok(PatchField::Unchanged),
        PatchField::Set(value) => map(value).map(PatchField::Set),
        PatchField::Clear => Ok(PatchField::Clear),
    }
}

fn configurations(
    values: Vec<AcquisitionConfigurationData>,
) -> Result<ListingSourceAcquisitionConfigurations, ApiError> {
    values
        .into_iter()
        .map(TryInto::try_into)
        .collect::<Result<Vec<_>, _>>()
        .map(ListingSourceAcquisitionConfigurations)
}

fn methods_data(values: HashSet<AcquisitionMethod>) -> Vec<AcquisitionMethodData> {
    let mut values = values.into_iter().collect::<Vec<_>>();
    values.sort_by_key(|value| value.as_str());
    values.into_iter().map(Into::into).collect()
}

fn parse_currency(value: Option<String>) -> Result<Option<money::Currency>, ApiError> {
    value
        .map(|value| money::Currency::from_code(&value).ok_or_else(|| invalid_body("currency")))
        .transpose()
}

fn parse_language(value: Option<String>) -> Result<Option<localization::Language>, ApiError> {
    value
        .map(|value| {
            localization::Language::from_code(&value).ok_or_else(|| invalid_body("language"))
        })
        .transpose()
}

fn invalid_body(field: &str) -> ApiError {
    ApiError::bad_request(BAD_BODY_VALUE).with_detail(format!("Body field '{field}' is invalid."))
}

fn deserialize_acquisition_method<'de, D>(deserializer: D) -> Result<AcquisitionMethod, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = String::deserialize(deserializer)?;
    AcquisitionMethod::from_str(&value).map_err(serde::de::Error::custom)
}

fn deserialize_acquisition_methods<'de, D>(
    deserializer: D,
) -> Result<HashSet<AcquisitionMethod>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Vec::<String>::deserialize(deserializer)?
        .into_iter()
        .map(|value| AcquisitionMethod::from_str(&value).map_err(serde::de::Error::custom))
        .collect()
}

fn deserialize_patch_acquisition_methods<'de, D>(
    deserializer: D,
) -> Result<PatchValue<HashSet<AcquisitionMethod>>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Option::<Vec<String>>::deserialize(deserializer)?.map_or(Ok(PatchValue::Null), |values| {
        values
            .into_iter()
            .map(|value| AcquisitionMethod::from_str(&value).map_err(serde::de::Error::custom))
            .collect::<Result<HashSet<_>, _>>()
            .map(PatchValue::Value)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_decode_canonical_acquisition_method_values() -> Result<(), serde_json::Error> {
        let source: CreateListingSourceData = serde_json::from_str(
            r#"{
                "name":"Source",
                "operatorPartyId":"550e8400-e29b-41d4-a716-446655440000",
                "acquisitionMethods":["WEB_CRAWL","PARTNER_API"],
                "acquisitionConfiguration":[{"type":"WEB_CRAWL"},{"type":"PARTNER_API"}]
            }"#,
        )?;

        assert_eq!(2, source.acquisition_methods.len());
        assert!(
            source
                .acquisition_methods
                .contains(&AcquisitionMethod::WebCrawl)
        );
        assert!(
            source
                .acquisition_methods
                .contains(&AcquisitionMethod::PartnerApi)
        );
        Ok(())
    }
}
