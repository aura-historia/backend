use domain_primitives::change_outcome::ChangeOutcome;
use party_core::party_id::PartyId;
use percent_encoding::{AsciiSet, CONTROLS, utf8_percent_encode};
use std::{
    collections::HashSet,
    fmt::{Display, Formatter},
    str::FromStr,
};
use strum::IntoEnumIterator;
use strum_macros::EnumIter;
use url::Url;
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
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

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ListingSourceName(String);
impl From<&str> for ListingSourceName {
    fn from(value: &str) -> Self {
        Self(value.chars().take(255).collect())
    }
}
impl From<String> for ListingSourceName {
    fn from(value: String) -> Self {
        Self::from(value.as_str())
    }
}
impl AsRef<str> for ListingSourceName {
    fn as_ref(&self) -> &str {
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
        let host = Url::parse(value)
            .ok()
            .and_then(|url| url.host_str().map(str::to_owned))
            .or_else(|| {
                Url::parse(&format!("https://{value}"))
                    .ok()
                    .and_then(|url| url.host_str().map(str::to_owned))
            })
            .ok_or_else(|| InvalidDomain(value.to_owned()))?;
        if host.contains('.') {
            Ok(Self(host.to_ascii_lowercase()))
        } else {
            Err(InvalidDomain(value.to_owned()))
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
impl ReferralConfiguration {
    pub fn apply(&self, deeplink: &Url) -> Result<Url, ReferralUrlError> {
        match self {
            Self::Partnerize { camref } => Url::parse(&format!(
                "https://prf.hn/click/camref:{camref}/pubref:aurahistoria/destination:{}",
                utf8_percent_encode(deeplink.as_str(), PARTNERIZE_DESTINATION)
            ))
            .map_err(|_| ReferralUrlError),
        }
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
    pub slug_id: ListingSourceSlugId,
    pub name: ListingSourceName,
    pub operator_party_id: PartyId,
    pub acquisition_methods: HashSet<AcquisitionMethod>,
    pub presentation: ListingSourcePresentation,
    pub referral_configuration: Option<ReferralConfiguration>,
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
        Self {
            slug_id: ListingSourceSlugId::from(input.name.as_ref()),
            id: input.id,
            name: input.name,
            operator_party_id: input.operator_party_id,
            acquisition_methods: input.acquisition_methods,
            presentation: input.presentation,
            referral_configuration: input.referral_configuration,
        }
    }
    #[doc(hidden)]
    pub fn rehydrate(state: RehydratedListingSourceState) -> Self {
        Self {
            id: state.id,
            slug_id: state.slug_id,
            name: state.name,
            operator_party_id: state.operator_party_id,
            acquisition_methods: state.acquisition_methods,
            presentation: state.presentation,
            referral_configuration: state.referral_configuration,
        }
    }
    pub fn rename(&mut self, name: ListingSourceName) -> ChangeOutcome {
        replace(&mut self.name, name)
    }
    pub fn change_operator(&mut self, party_id: PartyId) -> ChangeOutcome {
        replace(&mut self.operator_party_id, party_id)
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
        match &self.referral_configuration {
            Some(config) => config.apply(deeplink),
            None => Ok(append_utm(deeplink.clone())),
        }
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
fn append_utm(mut url: Url) -> Url {
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
    #[test]
    fn should_preserve_slug_and_apply_referral_to_deeplink() {
        let party = PartyId::new();
        let mut source = ListingSource::create(NewListingSource {
            id: ListingSourceId::new(),
            name: ListingSourceName::from("Old name"),
            operator_party_id: party,
            acquisition_methods: HashSet::from([AcquisitionMethod::WebCrawl]),
            presentation: ListingSourcePresentation::default(),
            referral_configuration: Some(ReferralConfiguration::Partnerize { camref: "x".into() }),
        });
        source.rename(ListingSourceName::from("New name"));
        let url = Url::parse("https://source.example/item")
            .unwrap_or_else(|error| panic!("test URL: {error}"));
        assert_eq!("old-name", source.slug_id().as_ref());
        assert!(
            source
                .referral_url(&url)
                .map(|value| value.as_str().contains("source.example%2Fitem"))
                .unwrap_or(false)
        );
    }
    #[test]
    fn should_parse_only_exact_acquisition_values() {
        assert_eq!(Ok(AcquisitionMethod::WebCrawl), "WEB_CRAWL".parse());
        assert!("web_crawl".parse::<AcquisitionMethod>().is_err());
    }
}
