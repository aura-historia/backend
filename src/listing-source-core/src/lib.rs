use domain_primitives::change_outcome::ChangeOutcome;
use party_core::party_id::PartyId;
use percent_encoding::{AsciiSet, CONTROLS, utf8_percent_encode};
use std::{
    collections::HashSet,
    fmt::{Display, Formatter},
    ops::Deref,
    str::FromStr,
};
use strum::IntoEnumIterator;
use strum_macros::EnumIter;
use url::Url;
use uuid::Uuid;

#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(into = "String", try_from = "String")]
pub struct ListingSourceId(Uuid);
impl ListingSourceId {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}
impl Default for ListingSourceId {
    fn default() -> Self {
        Self::new()
    }
}
impl Display for ListingSourceId {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}
impl From<Uuid> for ListingSourceId {
    fn from(value: Uuid) -> Self {
        Self(value)
    }
}
impl From<ListingSourceId> for Uuid {
    fn from(value: ListingSourceId) -> Self {
        value.0
    }
}
impl TryFrom<String> for ListingSourceId {
    type Error = uuid::Error;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Uuid::parse_str(&value).map(Self)
    }
}
impl TryFrom<&str> for ListingSourceId {
    type Error = uuid::Error;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        Uuid::parse_str(value).map(Self)
    }
}
impl From<ListingSourceId> for String {
    fn from(value: ListingSourceId) -> Self {
        value.0.to_string()
    }
}

/// Canonical ListingSource name, normalized by trimming Unicode whitespace at both ends.
///
/// Names must be nonblank and contain at most 255 UTF-8 bytes. The limit matches
/// the authoritative PostgreSQL constraint and values are never truncated.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ListingSourceName(String);

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ListingSourceNameError {
    #[error("listing source name must not be blank")]
    Blank,
    #[error("listing source name must not exceed {max_bytes} UTF-8 bytes (got {actual_bytes})")]
    TooLong {
        max_bytes: usize,
        actual_bytes: usize,
    },
}

impl ListingSourceName {
    pub const MAX_BYTES: usize = 255;
}

impl TryFrom<&str> for ListingSourceName {
    type Error = ListingSourceNameError;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        let value = value.trim();
        if value.is_empty() {
            return Err(ListingSourceNameError::Blank);
        }

        let actual_bytes = value.len();
        if actual_bytes > Self::MAX_BYTES {
            return Err(ListingSourceNameError::TooLong {
                max_bytes: Self::MAX_BYTES,
                actual_bytes,
            });
        }

        Ok(Self(value.to_owned()))
    }
}

impl TryFrom<String> for ListingSourceName {
    type Error = ListingSourceNameError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::try_from(value.as_str())
    }
}

impl AsRef<str> for ListingSourceName {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl Deref for ListingSourceName {
    type Target = str;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl Display for ListingSourceName {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, thiserror::Error)]
#[error("invalid listing source slug '{value}'")]
pub struct InvalidListingSourceSlug {
    value: String,
}
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ListingSourceSlugId(String);
impl ListingSourceSlugId {
    pub const FALLBACK_PREFIX: &str = "listing-source";

    pub fn raw(value: impl AsRef<str>) -> Result<Self, InvalidListingSourceSlug> {
        let value = value.as_ref();
        if !value.is_empty()
            && !value.starts_with('-')
            && !value.ends_with('-')
            && !value.contains("--")
            && value
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        {
            Ok(Self(value.to_owned()))
        } else {
            Err(InvalidListingSourceSlug {
                value: value.to_owned(),
            })
        }
    }

    pub(crate) fn derive(name: &str, listing_source_id: ListingSourceId) -> Self {
        match Self::raw(slug::slugify(name)) {
            Ok(slug_id) => slug_id,
            Err(_) => Self(format!("{}-{listing_source_id}", Self::FALLBACK_PREFIX)),
        }
    }
}
impl From<&str> for ListingSourceSlugId {
    fn from(value: &str) -> Self {
        Self(slug::slugify(value))
    }
}
impl AsRef<str> for ListingSourceSlugId {
    fn as_ref(&self) -> &str {
        &self.0
    }
}
impl Display for ListingSourceSlugId {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, thiserror::Error)]
#[error("invalid domain '{0}'")]
pub struct InvalidDomain(String);
impl InvalidDomain {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Domain(String);
impl Domain {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}
impl TryFrom<&str> for Domain {
    type Error = InvalidDomain;
    fn try_from(value: &str) -> Result<Self, Self::Error> {
        let trimmed = value.trim();
        if trimmed.is_empty() || trimmed != value || trimmed.contains(['/', '?', '#', '@', ':']) {
            return Err(InvalidDomain::new(value));
        }
        let url =
            Url::parse(&format!("https://{trimmed}")).map_err(|_| InvalidDomain::new(value))?;
        let host = url.host_str().ok_or_else(|| InvalidDomain::new(value))?;
        if host.contains('.') && url.port().is_none() {
            Ok(Self(host.to_ascii_lowercase()))
        } else {
            Err(InvalidDomain::new(value))
        }
    }
}
impl TryFrom<String> for Domain {
    type Error = InvalidDomain;
    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::try_from(value.as_str())
    }
}
impl Display for Domain {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, EnumIter)]
pub enum AcquisitionMethod {
    WebCrawl,
    Shopify,
    Woocommerce,
    PartnerApi,
}
impl AcquisitionMethod {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::WebCrawl => "WEB_CRAWL",
            Self::Shopify => "SHOPIFY",
            Self::Woocommerce => "WOOCOMMERCE",
            Self::PartnerApi => "PARTNER_API",
        }
    }
}
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("invalid acquisition method '{value}'")]
pub struct InvalidAcquisitionMethod {
    value: String,
}
impl FromStr for AcquisitionMethod {
    type Err = InvalidAcquisitionMethod;
    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::iter()
            .find(|method| method.as_str() == value)
            .ok_or_else(|| InvalidAcquisitionMethod {
                value: value.to_owned(),
            })
    }
}

const PARTNERIZE_DESTINATION: &AsciiSet = &CONTROLS
    .add(b' ')
    .add(b'"')
    .add(b'<')
    .add(b'>')
    .add(b'`')
    .add(b'#')
    .add(b'%')
    .add(b'?')
    .add(b'{')
    .add(b'}')
    .add(b'/')
    .add(b':')
    .add(b'@')
    .add(b'!')
    .add(b'$')
    .add(b'&')
    .add(b'\'')
    .add(b'(')
    .add(b')')
    .add(b'*')
    .add(b'+')
    .add(b',')
    .add(b';')
    .add(b'=')
    .add(b'[')
    .add(b']');
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReferralConfiguration {
    Partnerize { camref: String },
}
#[derive(Debug, thiserror::Error)]
#[error("could not build referral URL")]
pub struct ReferralUrlError;
pub fn outbound_url(
    referral_configuration: Option<&ReferralConfiguration>,
    destination: &Url,
) -> Result<Url, ReferralUrlError> {
    match referral_configuration {
        Some(ReferralConfiguration::Partnerize { camref }) => Url::parse(&format!(
            "https://prf.hn/click/camref:{camref}/pubref:aurahistoria/destination:{}",
            utf8_percent_encode(destination.as_str(), PARTNERIZE_DESTINATION)
        ))
        .map_err(|_| ReferralUrlError),
        None => Ok(append_aura_utm(destination.clone())),
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ListingSourcePresentation {
    pub url: Option<Url>,
    pub image: Option<Url>,
}
#[derive(Debug, Clone, PartialEq)]
pub struct NewListingSource {
    pub id: ListingSourceId,
    pub name: ListingSourceName,
    pub operator_party_id: PartyId,
    pub acquisition_methods: HashSet<AcquisitionMethod>,
    pub presentation: ListingSourcePresentation,
    pub referral_configuration: Option<ReferralConfiguration>,
}
#[doc(hidden)]
#[derive(Debug, Clone, PartialEq)]
pub struct RehydratedListingSourceState {
    pub id: ListingSourceId,
    pub slug_id: String,
    pub name: String,
    pub operator_party_id: PartyId,
    pub acquisition_methods: HashSet<AcquisitionMethod>,
    pub presentation: ListingSourcePresentation,
    pub referral_configuration: Option<ReferralConfiguration>,
}
#[derive(Debug, thiserror::Error)]
pub enum RehydrateListingSourceError {
    #[error("invalid persisted listing source slug")]
    InvalidSlug(#[source] InvalidListingSourceSlug),
    #[error("invalid persisted listing source name")]
    InvalidName(#[source] ListingSourceNameError),
}

#[derive(Debug, Clone, PartialEq)]
pub struct ListingSource {
    id: ListingSourceId,
    slug_id: ListingSourceSlugId,
    name: ListingSourceName,
    operator_party_id: PartyId,
    acquisition_methods: HashSet<AcquisitionMethod>,
    presentation: ListingSourcePresentation,
    referral_configuration: Option<ReferralConfiguration>,
}
impl ListingSource {
    pub fn create(input: NewListingSource) -> Self {
        let slug_id = ListingSourceSlugId::derive(input.name.as_ref(), input.id);
        Self {
            slug_id,
            id: input.id,
            name: input.name,
            operator_party_id: input.operator_party_id,
            acquisition_methods: input.acquisition_methods,
            presentation: input.presentation,
            referral_configuration: input.referral_configuration,
        }
    }
    #[doc(hidden)]
    pub fn rehydrate(
        state: RehydratedListingSourceState,
    ) -> Result<Self, RehydrateListingSourceError> {
        Ok(Self {
            id: state.id,
            slug_id: ListingSourceSlugId::raw(state.slug_id)
                .map_err(RehydrateListingSourceError::InvalidSlug)?,
            name: ListingSourceName::try_from(state.name)
                .map_err(RehydrateListingSourceError::InvalidName)?,
            operator_party_id: state.operator_party_id,
            acquisition_methods: state.acquisition_methods,
            presentation: state.presentation,
            referral_configuration: state.referral_configuration,
        })
    }
    pub fn rename(&mut self, name: ListingSourceName) -> ChangeOutcome {
        replace(&mut self.name, name)
    }

    pub fn replace_acquisition_methods(
        &mut self,
        methods: HashSet<AcquisitionMethod>,
    ) -> ChangeOutcome {
        replace(&mut self.acquisition_methods, methods)
    }
    pub fn replace_presentation(
        &mut self,
        presentation: ListingSourcePresentation,
    ) -> ChangeOutcome {
        replace(&mut self.presentation, presentation)
    }
    pub fn replace_referral_configuration(
        &mut self,
        config: Option<ReferralConfiguration>,
    ) -> ChangeOutcome {
        replace(&mut self.referral_configuration, config)
    }
    pub fn referral_url(&self, deeplink: &Url) -> Result<Url, ReferralUrlError> {
        outbound_url(self.referral_configuration.as_ref(), deeplink)
    }
    pub fn id(&self) -> ListingSourceId {
        self.id
    }
    pub fn slug_id(&self) -> &ListingSourceSlugId {
        &self.slug_id
    }
    pub fn name(&self) -> &ListingSourceName {
        &self.name
    }
    pub fn operator_party_id(&self) -> PartyId {
        self.operator_party_id
    }
    pub fn acquisition_methods(&self) -> &HashSet<AcquisitionMethod> {
        &self.acquisition_methods
    }
    pub fn presentation(&self) -> &ListingSourcePresentation {
        &self.presentation
    }
    pub fn referral_configuration(&self) -> Option<&ReferralConfiguration> {
        self.referral_configuration.as_ref()
    }
}
fn replace<T: PartialEq>(current: &mut T, replacement: T) -> ChangeOutcome {
    if *current == replacement {
        ChangeOutcome::Unchanged
    } else {
        *current = replacement;
        ChangeOutcome::Changed
    }
}
fn append_aura_utm(mut url: Url) -> Url {
    if !url.query_pairs().any(|(key, _)| key == "utm_source") {
        url.query_pairs_mut()
            .append_pair("utm_source", "aura_historia")
            .append_pair("utm_medium", "referral");
    }
    url
}

#[cfg(test)]
mod tests {
    use super::*;

    fn listing_source_name(value: &str) -> ListingSourceName {
        ListingSourceName::try_from(value)
            .unwrap_or_else(|error| panic!("invalid test listing source name: {error}"))
    }

    #[test]
    fn should_trim_unicode_outer_whitespace_from_listing_source_name() {
        let name = ListingSourceName::try_from("\u{2003} Antik und Stil \u{00a0}");

        assert_eq!(
            Ok("Antik und Stil".to_owned()),
            name.map(|value| value.to_string())
        );
    }

    #[test]
    fn should_reject_blank_listing_source_name_after_unicode_trim() {
        assert_eq!(
            Err(ListingSourceNameError::Blank),
            ListingSourceName::try_from("\u{2003}\u{00a0}")
        );
    }

    #[test]
    fn should_reject_listing_source_name_over_byte_cap_without_truncating() {
        let value = "é".repeat(128);

        assert_eq!(
            Err(ListingSourceNameError::TooLong {
                max_bytes: ListingSourceName::MAX_BYTES,
                actual_bytes: 256,
            }),
            ListingSourceName::try_from(value.as_str())
        );
    }

    #[test]
    fn should_accept_listing_source_name_at_byte_cap_without_truncating() {
        let value = format!("{}a", "é".repeat(127));
        let name = ListingSourceName::try_from(value.as_str());

        assert_eq!(Ok(value), name.map(|value| value.to_string()));
    }

    #[test]
    fn should_derive_slug_once_when_creating_listing_source() {
        let mut source = ListingSource::create(NewListingSource {
            id: ListingSourceId::new(),
            name: listing_source_name("Antik und Stil"),
            operator_party_id: PartyId::new(),
            acquisition_methods: HashSet::new(),
            presentation: ListingSourcePresentation::default(),
            referral_configuration: None,
        });

        assert_eq!("antik-und-stil", source.slug_id().as_ref());
        assert!(
            source
                .rename(listing_source_name("Neue Identität"))
                .changed()
        );
        assert_eq!("antik-und-stil", source.slug_id().as_ref());
    }

    #[test]
    fn should_use_stable_fallback_slug_when_listing_source_name_has_no_slug_characters() {
        let listing_source_id = ListingSourceId::from(Uuid::nil());
        let source = ListingSource::create(NewListingSource {
            id: listing_source_id,
            name: listing_source_name("\u{10FFFF}"),
            operator_party_id: PartyId::new(),
            acquisition_methods: HashSet::new(),
            presentation: ListingSourcePresentation::default(),
            referral_configuration: None,
        });

        assert_eq!(
            "listing-source-00000000-0000-0000-0000-000000000000",
            source.slug_id().as_ref()
        );
    }

    #[test]
    fn should_reject_invalid_persisted_listing_source_name_and_slug() {
        let state = RehydratedListingSourceState {
            id: ListingSourceId::new(),
            slug_id: "historic--source".to_owned(),
            name: "\u{2003}".to_owned(),
            operator_party_id: PartyId::new(),
            acquisition_methods: HashSet::new(),
            presentation: ListingSourcePresentation::default(),
            referral_configuration: None,
        };

        assert!(matches!(
            ListingSource::rehydrate(state),
            Err(RehydrateListingSourceError::InvalidSlug(_))
        ));

        let state = RehydratedListingSourceState {
            id: ListingSourceId::new(),
            slug_id: "historic-source".to_owned(),
            name: "\u{2003}".to_owned(),
            operator_party_id: PartyId::new(),
            acquisition_methods: HashSet::new(),
            presentation: ListingSourcePresentation::default(),
            referral_configuration: None,
        };

        assert!(matches!(
            ListingSource::rehydrate(state),
            Err(RehydrateListingSourceError::InvalidName(_))
        ));
    }

    #[test]
    fn should_build_partnerize_outbound_url_when_configured() {
        let destination = Url::parse("https://source.example/item?colour=red")
            .unwrap_or_else(|error| panic!("test URL: {error}"));

        let result = outbound_url(
            Some(&ReferralConfiguration::Partnerize {
                camref: "campaign".to_owned(),
            }),
            &destination,
        )
        .unwrap_or_else(|error| panic!("could not build referral URL: {error}"));

        assert_eq!(
            Url::parse(
                "https://prf.hn/click/camref:campaign/pubref:aurahistoria/destination:https%3A%2F%2Fsource.example%2Fitem%3Fcolour%3Dred",
            )
            .unwrap_or_else(|error| panic!("test URL: {error}")),
            result
        );
    }

    #[test]
    fn should_build_aura_utm_outbound_url_when_referral_is_not_configured() {
        let destination = Url::parse("https://source.example/item?colour=red")
            .unwrap_or_else(|error| panic!("test URL: {error}"));

        let result = outbound_url(None, &destination)
            .unwrap_or_else(|error| panic!("could not build referral URL: {error}"));

        assert_eq!(
            Url::parse(
                "https://source.example/item?colour=red&utm_source=aura_historia&utm_medium=referral",
            )
            .unwrap_or_else(|error| panic!("test URL: {error}")),
            result
        );
    }
    #[test]
    fn should_parse_only_exact_acquisition_values() {
        assert_eq!(Ok(AcquisitionMethod::WebCrawl), "WEB_CRAWL".parse());
        assert!("web_crawl".parse::<AcquisitionMethod>().is_err());
    }
}
