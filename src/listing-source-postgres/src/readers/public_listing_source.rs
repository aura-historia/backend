use crate::persisted_referral::{self, PersistedReferralConfigurationError};
use domain_primitives::object_id::ObjectIdError;
use listing_source_core::{
    ListingSourceId, ListingSourceName, ListingSourceSlugId, ReferralConfiguration,
    ReferralUrlError, outbound_url,
};
use listing_source_service::use_cases::queries::public_listing_source::{
    PublicListingSourceOperatorSummary, PublicListingSourceSummary, PublicListingSourceWebsite,
};
use party_core::party_name::PartyName;
use url::Url;

#[derive(Debug, sqlx::FromRow)]
pub(super) struct PublicListingSourceRow {
    pub(super) listing_source_id: uuid::Uuid,
    pub(super) listing_source_slug_id: String,
    pub(super) name: String,
    pub(super) operator_name: String,
    pub(super) operator_partner: bool,
    pub(super) url: Option<String>,
    pub(super) image: Option<String>,
    pub(super) referral_configuration: Option<serde_json::Value>,
}

#[derive(Debug, thiserror::Error)]
pub(super) enum PublicListingSourceMappingError {
    #[error("invalid public ListingSource ID persisted")]
    ListingSourceId(#[source] ObjectIdError),
    #[error("invalid public ListingSource slug persisted")]
    ListingSourceSlug(#[source] listing_source_core::InvalidListingSourceSlug),
    #[error("invalid public ListingSource name persisted")]
    ListingSourceName(#[source] listing_source_core::ListingSourceNameError),
    #[error("invalid public operator name persisted")]
    OperatorName(#[source] party_core::party_name::PartyNameError),
    #[error("invalid public presentation URL persisted")]
    Url(#[source] url::ParseError),
    #[error("unsafe public presentation URL persisted")]
    UnsafeUrl,
    #[error("invalid public ListingSource referral configuration persisted")]
    ReferralConfiguration(#[source] PersistedReferralConfigurationError),
    #[error("public ListingSource view URL could not be built")]
    ViewUrl(#[source] ReferralUrlError),
}

pub(super) fn map_public_listing_source(
    row: PublicListingSourceRow,
) -> Result<PublicListingSourceSummary, PublicListingSourceMappingError> {
    let referral_configuration = persisted_referral::from_json(row.referral_configuration.as_ref())
        .map_err(PublicListingSourceMappingError::ReferralConfiguration)?;
    Ok(PublicListingSourceSummary {
        listing_source_id: ListingSourceId::try_from(row.listing_source_id)
            .map_err(PublicListingSourceMappingError::ListingSourceId)?,
        listing_source_slug_id: ListingSourceSlugId::raw(row.listing_source_slug_id)
            .map_err(PublicListingSourceMappingError::ListingSourceSlug)?,
        name: ListingSourceName::try_from(row.name)
            .map_err(PublicListingSourceMappingError::ListingSourceName)?,
        operator: PublicListingSourceOperatorSummary {
            name: PartyName::try_from(row.operator_name)
                .map_err(PublicListingSourceMappingError::OperatorName)?,
            partner: row.operator_partner,
        },
        website: map_website(row.url, referral_configuration.as_ref())?,
        image: map_public_url(row.image)?,
    })
}

fn map_website(
    url: Option<String>,
    referral_configuration: Option<&ReferralConfiguration>,
) -> Result<Option<PublicListingSourceWebsite>, PublicListingSourceMappingError> {
    map_public_url(url)?
        .map(|url| {
            let view_url = outbound_url(referral_configuration, &url)
                .map_err(PublicListingSourceMappingError::ViewUrl)?;
            Ok(PublicListingSourceWebsite { url, view_url })
        })
        .transpose()
}

fn map_public_url(value: Option<String>) -> Result<Option<Url>, PublicListingSourceMappingError> {
    value
        .map(|value| {
            let url = Url::parse(&value).map_err(PublicListingSourceMappingError::Url)?;
            if !matches!(url.scheme(), "http" | "https")
                || !url.username().is_empty()
                || url.password().is_some()
            {
                return Err(PublicListingSourceMappingError::UnsafeUrl);
            }
            Ok(url)
        })
        .transpose()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(url: Option<&str>, image: Option<&str>) -> PublicListingSourceRow {
        PublicListingSourceRow {
            listing_source_id: uuid::Uuid::now_v7(),
            listing_source_slug_id: "source".to_owned(),
            name: "Source".to_owned(),
            operator_name: "Operator".to_owned(),
            operator_partner: false,
            url: url.map(ToOwned::to_owned),
            image: image.map(ToOwned::to_owned),
            referral_configuration: None,
        }
    }

    fn partnerize(camref: &str) -> serde_json::Value {
        serde_json::json!({"kind":"PARTNERIZE","camref":camref})
    }

    fn website_of(summary: PublicListingSourceSummary) -> PublicListingSourceWebsite {
        summary
            .website
            .unwrap_or_else(|| panic!("source should have a website"))
    }

    #[test]
    fn should_accept_safe_public_presentation_urls() {
        let summary = map_public_listing_source(row(
            Some("https://example.test/source"),
            Some("http://example.test/image"),
        ));

        assert!(summary.is_ok());
    }

    #[test]
    fn should_tag_view_url_with_aura_utm_when_source_has_no_referral_configuration() {
        let summary = map_public_listing_source(row(Some("https://example.test/source"), None))
            .unwrap_or_else(|error| panic!("map public source: {error}"));

        let website = website_of(summary);

        assert_eq!("https://example.test/source", website.url.as_str());
        assert_eq!(
            "https://example.test/source?utm_source=aura_historia&utm_medium=referral",
            website.view_url.as_str()
        );
    }

    #[test]
    fn should_route_view_url_through_partnerize_when_source_has_referral_configuration() {
        let mut referred = row(Some("https://example.test/source"), None);
        referred.referral_configuration = Some(partnerize("1101l3AbC"));

        let website = website_of(
            map_public_listing_source(referred)
                .unwrap_or_else(|error| panic!("map public source: {error}")),
        );

        assert_eq!("https://example.test/source", website.url.as_str());
        assert_eq!(
            "https://prf.hn/click/camref:1101l3AbC/pubref:aurahistoria/destination:https%3A%2F%2Fexample.test%2Fsource",
            website.view_url.as_str()
        );
    }

    #[test]
    fn should_omit_website_when_source_has_no_url() {
        let mut referred = row(None, Some("https://example.test/image"));
        referred.referral_configuration = Some(partnerize("1101l3AbC"));

        let summary = map_public_listing_source(referred)
            .unwrap_or_else(|error| panic!("map public source: {error}"));

        assert_eq!(None, summary.website);
    }

    #[test]
    fn should_reject_invalid_persisted_referral_configuration_even_without_url() {
        let mut invalid = row(None, None);
        invalid.referral_configuration = Some(partnerize("campaign/ref"));

        assert!(matches!(
            map_public_listing_source(invalid),
            Err(PublicListingSourceMappingError::ReferralConfiguration(_))
        ));
    }

    #[test]
    fn should_reject_unsafe_public_presentation_urls() {
        for value in [
            "ftp://example.test/source",
            "https://user@example.test/source",
            "https://user:password@example.test/source",
        ] {
            assert!(matches!(
                map_public_listing_source(row(Some(value), None)),
                Err(PublicListingSourceMappingError::UnsafeUrl)
            ));
        }
    }

    #[test]
    fn should_reject_wrong_version_public_listing_source_id() {
        let mut invalid = row(None, None);
        invalid.listing_source_id = uuid::Uuid::new_v4();

        assert!(matches!(
            map_public_listing_source(invalid),
            Err(PublicListingSourceMappingError::ListingSourceId(_))
        ));
    }
}
