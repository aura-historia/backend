use indexmap::IndexSet;
use listing_source_core::ListingSourceId;
use localization::{Language, Localized};
use money::{Currency, MonetaryAmount, Price};
use product_listing_core::{
    description::Description,
    listing_availability::ListingAvailability,
    listing_lifecycle::ListingLifecycle,
    product_listing::{
        ListingSaleObservation, NewProductListing, ProductListing, ProductListingAuction,
        ProductListingPricing, RehydratedProductListingState,
    },
    product_listing_event::{
        ListingSaleObservationChange, ProductListingChanged, ProductListingDiscovered,
        ProductListingEventPayload, ProductListingEventType, ProductListingLifecycleChange,
        ValueChange,
    },
    product_listing_id::ProductListingId,
    product_listing_image::ProductListingImage,
    product_listing_slug_id::ProductListingSlugId,
    source_listing_id::SourceListingId,
    title::Title,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use strum::IntoEnumIterator;
use time::{OffsetDateTime, format_description::well_known::Rfc3339};
use url::Url;

pub(crate) const PRODUCT_LISTING_EVENT_SCHEMA_VERSION: i16 = 1;

#[derive(Debug, thiserror::Error)]
pub(crate) enum ProductListingEventCodecError {
    #[error("product listing event payload is invalid")]
    Invalid,
}

pub(crate) fn encode(
    payload: &ProductListingEventPayload,
) -> Result<Value, ProductListingEventCodecError> {
    let value = match payload {
        ProductListingEventPayload::Discovered(discovered) => {
            serde_json::to_value(DiscoveredDto::try_from(discovered)?)
        }
        ProductListingEventPayload::Changed(changed) => {
            serde_json::to_value(ChangedDto::try_from(changed)?)
        }
    };

    value.map_err(|_| ProductListingEventCodecError::Invalid)
}

pub(crate) fn decode(
    event_type: &str,
    schema_version: i16,
    payload: &Value,
) -> Result<ProductListingEventPayload, ProductListingEventCodecError> {
    if schema_version != PRODUCT_LISTING_EVENT_SCHEMA_VERSION {
        return Err(ProductListingEventCodecError::Invalid);
    }

    let event_type = ProductListingEventType::iter()
        .find(|candidate| candidate.as_str() == event_type)
        .ok_or(ProductListingEventCodecError::Invalid)?;

    match event_type {
        ProductListingEventType::Discovered => {
            serde_json::from_value::<DiscoveredDto>(payload.clone())
                .map_err(|_| ProductListingEventCodecError::Invalid)
                .and_then(TryInto::try_into)
        }
        ProductListingEventType::Changed => serde_json::from_value::<ChangedDto>(payload.clone())
            .map_err(|_| ProductListingEventCodecError::Invalid)
            .and_then(|changed| changed.try_into_payload(payload)),
    }
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct DiscoveredDto {
    listing_source_id: String,
    source_listing_id: String,
    title: Option<LocalizedTextDto>,
    description: Option<LocalizedTextDto>,
    pricing: PricingDto,
    availability: Option<String>,
    url: String,
    image_count: usize,
    auction: AuctionDto,
}

impl TryFrom<&ProductListingDiscovered> for DiscoveredDto {
    type Error = ProductListingEventCodecError;

    fn try_from(value: &ProductListingDiscovered) -> Result<Self, Self::Error> {
        Ok(Self {
            listing_source_id: value.listing_source_id().to_string(),
            source_listing_id: value.source_listing_id().as_ref().to_owned(),
            title: value.title().map(LocalizedTextDto::from),
            description: value.description().map(LocalizedTextDto::from),
            pricing: value.pricing().into(),
            availability: value
                .availability()
                .map(|availability| availability.as_str().to_owned()),
            url: value.url().as_str().to_owned(),
            image_count: value.image_count(),
            auction: value.auction().try_into()?,
        })
    }
}

impl TryFrom<DiscoveredDto> for ProductListingEventPayload {
    type Error = ProductListingEventCodecError;

    fn try_from(value: DiscoveredDto) -> Result<Self, Self::Error> {
        let mut listing = ProductListing::create(NewProductListing {
            id: ProductListingId::new(),
            title_slug_id: codec_slug()?,
            listing_source_id: parse_id(&value.listing_source_id)?,
            source_listing_id: SourceListingId::try_from(value.source_listing_id)
                .map_err(|_| ProductListingEventCodecError::Invalid)?,
            title: value.title.map(TryInto::try_into).transpose()?,
            description: value.description.map(TryInto::try_into).transpose()?,
            pricing: value.pricing.try_into()?,
            availability: parse_availability(value.availability)?,
            url: parse_url(&value.url)?,
            images: images(value.image_count, "discovered")?,
            auction: value.auction.try_into()?,
        })
        .map_err(|_| ProductListingEventCodecError::Invalid)?;

        match listing.take_pending_event_payload() {
            Some(ProductListingEventPayload::Discovered(discovered)) => {
                Ok(ProductListingEventPayload::Discovered(discovered))
            }
            _ => Err(ProductListingEventCodecError::Invalid),
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ChangedDto {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pricing: Option<PricingChangesDto>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    availability: Option<ValueChangeDto<Option<String>>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    url: Option<ValueChangeDto<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    images: Option<ImageCountChangeDto>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    auction: Option<ValueChangeDto<AuctionDto>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    lifecycle: Option<LifecycleChangeDto>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    sale_observation: Option<SaleObservationChangeDto>,
}

impl TryFrom<&ProductListingChanged> for ChangedDto {
    type Error = ProductListingEventCodecError;

    fn try_from(value: &ProductListingChanged) -> Result<Self, Self::Error> {
        let pricing = PricingChangesDto {
            price: value.price().map(price_change_dto).transpose()?,
            price_estimate_min: value
                .price_estimate_min()
                .map(price_change_dto)
                .transpose()?,
            price_estimate_max: value
                .price_estimate_max()
                .map(price_change_dto)
                .transpose()?,
        };
        Ok(Self {
            pricing: (!pricing.is_empty()).then_some(pricing),
            availability: value.availability().map(availability_change_dto),
            url: value.url().map(url_change_dto),
            images: value.image_count().map(ImageCountChangeDto::from),
            auction: value.auction().map(auction_change_dto).transpose()?,
            lifecycle: value.lifecycle().map(LifecycleChangeDto::from),
            sale_observation: value
                .sale_observation()
                .map(SaleObservationChangeDto::try_from)
                .transpose()?,
        })
    }
}

impl ChangedDto {
    fn try_into_payload(
        self,
        raw_payload: &Value,
    ) -> Result<ProductListingEventPayload, ProductListingEventCodecError> {
        if self.is_empty() {
            return Err(ProductListingEventCodecError::Invalid);
        }
        self.validate_conditional_fields(raw_payload)?;

        let pricing = self
            .pricing
            .as_ref()
            .map(PricingChangesDto::previous)
            .transpose()?;
        let availability = self
            .availability
            .as_ref()
            .map(|change| parse_availability(change.previous.clone()))
            .transpose()?
            .flatten();
        let url = self
            .url
            .as_ref()
            .map(|change| parse_url(&change.previous))
            .transpose()?
            .unwrap_or(codec_url()?);
        let image_count = self
            .images
            .as_ref()
            .map_or(Ok(0), |change| Ok(change.previous_count))?;
        let auction = self
            .auction
            .as_ref()
            .map(|change| change.previous.clone().try_into())
            .transpose()?
            .unwrap_or_default();
        let lifecycle = self
            .lifecycle
            .as_ref()
            .map(ProductListingLifecycleChange::try_from)
            .transpose()?;
        let sale_observation = self
            .sale_observation
            .as_ref()
            .and_then(SaleObservationChangeDto::retracted_observation);

        let mut listing = ProductListing::rehydrate(RehydratedProductListingState {
            id: ProductListingId::new(),
            title_slug_id: codec_slug()?,
            listing_source_id: ListingSourceId::new(),
            source_listing_id: SourceListingId::try_from("codec-source")
                .map_err(|_| ProductListingEventCodecError::Invalid)?,
            title: None,
            description: None,
            pricing: pricing.unwrap_or_default(),
            sale_observation,
            availability: if matches!(
                lifecycle.as_ref(),
                Some(ProductListingLifecycleChange::Restored)
            ) {
                None
            } else {
                availability
            },
            lifecycle: if matches!(
                lifecycle.as_ref(),
                Some(ProductListingLifecycleChange::Restored)
            ) {
                ListingLifecycle::Withdrawn
            } else {
                ListingLifecycle::Active
            },
            url,
            images: images(image_count, "previous")?,
            auction,
        })
        .map_err(|_| ProductListingEventCodecError::Invalid)?;

        if matches!(
            lifecycle.as_ref(),
            Some(ProductListingLifecycleChange::Restored)
        ) {
            listing.restore();
        }
        if let Some(pricing) = self.pricing.as_ref() {
            listing
                .replace_pricing(pricing.current()?)
                .map_err(|_| ProductListingEventCodecError::Invalid)?;
        }
        let withdrawal_previous_availability = match lifecycle.as_ref() {
            Some(ProductListingLifecycleChange::Withdrawn {
                previous_availability,
            }) => Some(*previous_availability),
            _ => None,
        };
        if let Some(previous_availability) = withdrawal_previous_availability {
            apply_availability_value(&mut listing, previous_availability)?;
        } else if let Some(availability) = self.availability.as_ref() {
            apply_availability(&mut listing, availability.current.clone())?;
        }
        if let Some(url) = self.url.as_ref() {
            listing
                .change_url(parse_url(&url.current)?)
                .map_err(|_| ProductListingEventCodecError::Invalid)?;
        }
        if let Some(images_change) = self.images.as_ref() {
            listing
                .replace_images(images(images_change.current_count, "current")?)
                .map_err(|_| ProductListingEventCodecError::Invalid)?;
        }
        if let Some(auction) = self.auction.as_ref() {
            listing
                .replace_auction(auction.current.clone().try_into()?)
                .map_err(|_| ProductListingEventCodecError::Invalid)?;
        }
        if let Some(sale_observation) = self.sale_observation.as_ref() {
            sale_observation.apply(&mut listing)?;
        }
        if matches!(
            lifecycle.as_ref(),
            Some(ProductListingLifecycleChange::Withdrawn { .. })
        ) {
            listing.withdraw();
        }

        let payload = match listing.take_pending_event_payload() {
            Some(ProductListingEventPayload::Changed(changed)) => {
                ProductListingEventPayload::Changed(changed)
            }
            _ => return Err(ProductListingEventCodecError::Invalid),
        };
        if encode(&payload)? != *raw_payload {
            return Err(ProductListingEventCodecError::Invalid);
        }
        Ok(payload)
    }

    fn is_empty(&self) -> bool {
        self.pricing.is_none()
            && self.availability.is_none()
            && self.url.is_none()
            && self.images.is_none()
            && self.auction.is_none()
            && self.lifecycle.is_none()
            && self.sale_observation.is_none()
    }

    fn validate_conditional_fields(
        &self,
        raw_payload: &Value,
    ) -> Result<(), ProductListingEventCodecError> {
        let object = raw_payload
            .as_object()
            .ok_or(ProductListingEventCodecError::Invalid)?;
        if let Some(lifecycle) = self.lifecycle.as_ref() {
            let lifecycle_object = object
                .get("lifecycle")
                .and_then(Value::as_object)
                .ok_or(ProductListingEventCodecError::Invalid)?;
            match lifecycle.transition.as_str() {
                "WITHDRAWN" if lifecycle_object.contains_key("previousAvailability") => {}
                "RESTORED" if !lifecycle_object.contains_key("previousAvailability") => {}
                _ => return Err(ProductListingEventCodecError::Invalid),
            }
        }
        Ok(())
    }
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PricingChangesDto {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    price: Option<ValueChangeDto<Option<PriceDto>>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    price_estimate_min: Option<ValueChangeDto<Option<PriceDto>>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    price_estimate_max: Option<ValueChangeDto<Option<PriceDto>>>,
}

impl PricingChangesDto {
    fn is_empty(&self) -> bool {
        self.price.is_none()
            && self.price_estimate_min.is_none()
            && self.price_estimate_max.is_none()
    }

    fn previous(&self) -> Result<ProductListingPricing, ProductListingEventCodecError> {
        Ok(ProductListingPricing {
            price: self
                .price
                .as_ref()
                .map(|change| parse_price(change.previous.clone()))
                .transpose()?
                .flatten(),
            price_estimate_min: self
                .price_estimate_min
                .as_ref()
                .map(|change| parse_price(change.previous.clone()))
                .transpose()?
                .flatten(),
            price_estimate_max: self
                .price_estimate_max
                .as_ref()
                .map(|change| parse_price(change.previous.clone()))
                .transpose()?
                .flatten(),
        })
    }

    fn current(&self) -> Result<ProductListingPricing, ProductListingEventCodecError> {
        Ok(ProductListingPricing {
            price: self
                .price
                .as_ref()
                .map(|change| parse_price(change.current.clone()))
                .transpose()?
                .flatten(),
            price_estimate_min: self
                .price_estimate_min
                .as_ref()
                .map(|change| parse_price(change.current.clone()))
                .transpose()?
                .flatten(),
            price_estimate_max: self
                .price_estimate_max
                .as_ref()
                .map(|change| parse_price(change.current.clone()))
                .transpose()?
                .flatten(),
        })
    }
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ValueChangeDto<T> {
    previous: T,
    current: T,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ImageCountChangeDto {
    previous_count: usize,
    current_count: usize,
}

impl From<&ValueChange<usize>> for ImageCountChangeDto {
    fn from(value: &ValueChange<usize>) -> Self {
        Self {
            previous_count: *value.previous(),
            current_count: *value.current(),
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct LifecycleChangeDto {
    transition: String,
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    previous_availability: Option<Option<String>>,
}

impl From<&ProductListingLifecycleChange> for LifecycleChangeDto {
    fn from(value: &ProductListingLifecycleChange) -> Self {
        match value {
            ProductListingLifecycleChange::Withdrawn {
                previous_availability,
            } => Self {
                transition: "WITHDRAWN".to_owned(),
                previous_availability: Some(
                    previous_availability.map(|value| value.as_str().to_owned()),
                ),
            },
            ProductListingLifecycleChange::Restored => Self {
                transition: "RESTORED".to_owned(),
                previous_availability: None,
            },
        }
    }
}

impl TryFrom<&LifecycleChangeDto> for ProductListingLifecycleChange {
    type Error = ProductListingEventCodecError;

    fn try_from(value: &LifecycleChangeDto) -> Result<Self, Self::Error> {
        match value.transition.as_str() {
            "WITHDRAWN" => Ok(Self::Withdrawn {
                previous_availability: parse_availability(
                    value.previous_availability.clone().flatten(),
                )?,
            }),
            "RESTORED" => Ok(Self::Restored),
            _ => Err(ProductListingEventCodecError::Invalid),
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SaleObservationChangeDto {
    transition: String,
    observation: SaleObservationDto,
}

impl TryFrom<&ListingSaleObservationChange> for SaleObservationChangeDto {
    type Error = ProductListingEventCodecError;

    fn try_from(value: &ListingSaleObservationChange) -> Result<Self, Self::Error> {
        match value {
            ListingSaleObservationChange::Observed(observation) => Ok(Self {
                transition: "OBSERVED".to_owned(),
                observation: (*observation).try_into()?,
            }),
            ListingSaleObservationChange::Retracted(observation) => Ok(Self {
                transition: "RETRACTED".to_owned(),
                observation: (*observation).try_into()?,
            }),
        }
    }
}

impl SaleObservationChangeDto {
    fn retracted_observation(&self) -> Option<ListingSaleObservation> {
        (self.transition == "RETRACTED")
            .then(|| self.observation.clone().try_into().ok())
            .flatten()
    }

    fn apply(&self, listing: &mut ProductListing) -> Result<(), ProductListingEventCodecError> {
        let observation: ListingSaleObservation = self.observation.clone().try_into()?;
        match self.transition.as_str() {
            "OBSERVED" => listing
                .record_sale_observation(observation)
                .map(|_| ())
                .map_err(|_| ProductListingEventCodecError::Invalid),
            "RETRACTED" => {
                if listing.retract_sale_observation().changed() {
                    Ok(())
                } else {
                    Err(ProductListingEventCodecError::Invalid)
                }
            }
            _ => Err(ProductListingEventCodecError::Invalid),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct LocalizedTextDto {
    language: String,
    text: String,
}

impl From<&Localized<Language, Title>> for LocalizedTextDto {
    fn from(value: &Localized<Language, Title>) -> Self {
        Self {
            language: value.localization.as_str().to_owned(),
            text: value.payload.as_ref().to_owned(),
        }
    }
}

impl From<&Localized<Language, Description>> for LocalizedTextDto {
    fn from(value: &Localized<Language, Description>) -> Self {
        Self {
            language: value.localization.as_str().to_owned(),
            text: value.payload.as_ref().to_owned(),
        }
    }
}

impl TryFrom<LocalizedTextDto> for Localized<Language, Title> {
    type Error = ProductListingEventCodecError;

    fn try_from(value: LocalizedTextDto) -> Result<Self, Self::Error> {
        Ok(Localized::new(
            parse_language(&value.language)?,
            Title::from(value.text),
        ))
    }
}

impl TryFrom<LocalizedTextDto> for Localized<Language, Description> {
    type Error = ProductListingEventCodecError;

    fn try_from(value: LocalizedTextDto) -> Result<Self, Self::Error> {
        Ok(Localized::new(
            parse_language(&value.language)?,
            Description::from(value.text),
        ))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PricingDto {
    price: Option<PriceDto>,
    price_estimate_min: Option<PriceDto>,
    price_estimate_max: Option<PriceDto>,
}

impl From<ProductListingPricing> for PricingDto {
    fn from(value: ProductListingPricing) -> Self {
        Self {
            price: value.price.map(Into::into),
            price_estimate_min: value.price_estimate_min.map(Into::into),
            price_estimate_max: value.price_estimate_max.map(Into::into),
        }
    }
}

impl TryFrom<PricingDto> for ProductListingPricing {
    type Error = ProductListingEventCodecError;

    fn try_from(value: PricingDto) -> Result<Self, Self::Error> {
        Ok(Self {
            price: parse_price(value.price)?,
            price_estimate_min: parse_price(value.price_estimate_min)?,
            price_estimate_max: parse_price(value.price_estimate_max)?,
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PriceDto {
    amount: u64,
    currency: String,
}

impl From<Price> for PriceDto {
    fn from(value: Price) -> Self {
        Self {
            amount: u64::from(value.monetary_amount),
            currency: value.currency.as_str().to_owned(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct AuctionDto {
    start: Option<String>,
    end: Option<String>,
}

impl TryFrom<ProductListingAuction> for AuctionDto {
    type Error = ProductListingEventCodecError;

    fn try_from(value: ProductListingAuction) -> Result<Self, Self::Error> {
        Ok(Self {
            start: value.start.map(format_timestamp).transpose()?,
            end: value.end.map(format_timestamp).transpose()?,
        })
    }
}

impl TryFrom<AuctionDto> for ProductListingAuction {
    type Error = ProductListingEventCodecError;

    fn try_from(value: AuctionDto) -> Result<Self, Self::Error> {
        let auction = Self {
            start: value.start.as_deref().map(parse_timestamp).transpose()?,
            end: value.end.as_deref().map(parse_timestamp).transpose()?,
        };
        if auction
            .start
            .zip(auction.end)
            .is_some_and(|(start, end)| start > end)
        {
            return Err(ProductListingEventCodecError::Invalid);
        }
        Ok(auction)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SaleObservationDto {
    observed_at: String,
    fx_rate_id: String,
}

impl TryFrom<ListingSaleObservation> for SaleObservationDto {
    type Error = ProductListingEventCodecError;

    fn try_from(value: ListingSaleObservation) -> Result<Self, Self::Error> {
        Ok(Self {
            observed_at: format_timestamp(value.observed_at())?,
            fx_rate_id: value.fx_rate_id().to_string(),
        })
    }
}

impl TryFrom<SaleObservationDto> for ListingSaleObservation {
    type Error = ProductListingEventCodecError;

    fn try_from(value: SaleObservationDto) -> Result<Self, Self::Error> {
        let fx_rate_id = value
            .fx_rate_id
            .parse::<uuid::Uuid>()
            .map(fxrate_core::FxRateId::from)
            .map_err(|_| ProductListingEventCodecError::Invalid)?;
        Ok(Self::new(parse_timestamp(&value.observed_at)?, fx_rate_id))
    }
}

fn price_change_dto(
    value: &ValueChange<Option<Price>>,
) -> Result<ValueChangeDto<Option<PriceDto>>, ProductListingEventCodecError> {
    Ok(ValueChangeDto {
        previous: value.previous().map(PriceDto::from),
        current: value.current().map(PriceDto::from),
    })
}

fn availability_change_dto(
    value: &ValueChange<Option<ListingAvailability>>,
) -> ValueChangeDto<Option<String>> {
    ValueChangeDto {
        previous: value
            .previous()
            .map(|availability| availability.as_str().to_owned()),
        current: value
            .current()
            .map(|availability| availability.as_str().to_owned()),
    }
}

fn url_change_dto(value: &ValueChange<Url>) -> ValueChangeDto<String> {
    ValueChangeDto {
        previous: value.previous().as_str().to_owned(),
        current: value.current().as_str().to_owned(),
    }
}

fn auction_change_dto(
    value: &ValueChange<ProductListingAuction>,
) -> Result<ValueChangeDto<AuctionDto>, ProductListingEventCodecError> {
    Ok(ValueChangeDto {
        previous: (*value.previous()).try_into()?,
        current: (*value.current()).try_into()?,
    })
}

fn parse_id(value: &str) -> Result<ListingSourceId, ProductListingEventCodecError> {
    value
        .parse::<uuid::Uuid>()
        .map(ListingSourceId::from)
        .map_err(|_| ProductListingEventCodecError::Invalid)
}

fn parse_language(value: &str) -> Result<Language, ProductListingEventCodecError> {
    Language::from_code(value).ok_or(ProductListingEventCodecError::Invalid)
}

fn parse_availability(
    value: Option<String>,
) -> Result<Option<ListingAvailability>, ProductListingEventCodecError> {
    value
        .map(|value| {
            ListingAvailability::from_code(&value).ok_or(ProductListingEventCodecError::Invalid)
        })
        .transpose()
}

fn parse_price(value: Option<PriceDto>) -> Result<Option<Price>, ProductListingEventCodecError> {
    value
        .map(|value| {
            Currency::from_code(&value.currency)
                .map(|currency| Price::new(MonetaryAmount::from(value.amount), currency))
                .ok_or(ProductListingEventCodecError::Invalid)
        })
        .transpose()
}

fn parse_url(value: &str) -> Result<Url, ProductListingEventCodecError> {
    Url::parse(value).map_err(|_| ProductListingEventCodecError::Invalid)
}

fn format_timestamp(value: OffsetDateTime) -> Result<String, ProductListingEventCodecError> {
    value
        .format(&Rfc3339)
        .map_err(|_| ProductListingEventCodecError::Invalid)
}

fn parse_timestamp(value: &str) -> Result<OffsetDateTime, ProductListingEventCodecError> {
    OffsetDateTime::parse(value, &Rfc3339).map_err(|_| ProductListingEventCodecError::Invalid)
}

fn codec_slug() -> Result<ProductListingSlugId, ProductListingEventCodecError> {
    ProductListingSlugId::raw("codec-listing-a1b2c3")
        .map_err(|_| ProductListingEventCodecError::Invalid)
}

fn codec_url() -> Result<Url, ProductListingEventCodecError> {
    parse_url("https://codec.invalid/")
}

fn images(
    count: usize,
    prefix: &str,
) -> Result<IndexSet<ProductListingImage>, ProductListingEventCodecError> {
    (0..count)
        .map(|index| {
            parse_url(&format!("https://codec.invalid/{prefix}/{index}"))
                .map(ProductListingImage::new)
        })
        .collect()
}

fn apply_availability(
    listing: &mut ProductListing,
    availability: Option<String>,
) -> Result<(), ProductListingEventCodecError> {
    apply_availability_value(listing, parse_availability(availability)?)
}

fn apply_availability_value(
    listing: &mut ProductListing,
    availability: Option<ListingAvailability>,
) -> Result<(), ProductListingEventCodecError> {
    match availability {
        Some(availability) => listing
            .set_availability(availability)
            .map(|_| ())
            .map_err(|_| ProductListingEventCodecError::Invalid),
        None => listing
            .clear_availability()
            .map(|_| ())
            .map_err(|_| ProductListingEventCodecError::Invalid),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use time::Duration;

    #[test]
    fn should_round_trip_discovered_payload_without_kind_or_image_urls() {
        let payload = discovered_payload();
        let encoded = encode(&payload).unwrap_or_else(|error| panic!("encode: {error}"));

        assert!(encoded.get("kind").is_none());
        assert_eq!(Some(1), encoded.get("imageCount").and_then(Value::as_u64));
        assert!(!encoded.to_string().contains("image-a.jpg"));
        assert_eq!(
            payload,
            decode(
                ProductListingEventType::Discovered.as_str(),
                PRODUCT_LISTING_EVENT_SCHEMA_VERSION,
                &encoded,
            )
            .unwrap_or_else(|error| panic!("decode: {error}")),
        );
    }

    #[test]
    fn should_round_trip_changed_payload_with_all_dimensions() {
        let mut listing = sample_listing();
        listing.take_pending_event_payload();
        listing
            .replace_pricing(ProductListingPricing {
                price: Some(price(20)),
                price_estimate_min: Some(price(15)),
                price_estimate_max: Some(price(25)),
            })
            .unwrap_or_else(|error| panic!("pricing: {error}"));
        listing
            .clear_availability()
            .unwrap_or_else(|error| panic!("availability: {error}"));
        listing
            .change_url(
                parse_url("https://example.com/new").unwrap_or_else(|error| panic!("URL: {error}")),
            )
            .unwrap_or_else(|error| panic!("URL change: {error}"));
        listing
            .replace_images(images(2, "test").unwrap_or_else(|error| panic!("images: {error}")))
            .unwrap_or_else(|error| panic!("image change: {error}"));
        listing
            .replace_auction(ProductListingAuction {
                start: Some(OffsetDateTime::UNIX_EPOCH),
                end: Some(OffsetDateTime::UNIX_EPOCH + Duration::hours(1)),
            })
            .unwrap_or_else(|error| panic!("auction: {error}"));
        listing
            .record_sale_observation(ListingSaleObservation::new(
                OffsetDateTime::UNIX_EPOCH,
                fxrate_core::FxRateId::new(),
            ))
            .unwrap_or_else(|error| panic!("sale observation: {error}"));
        listing.withdraw();
        let payload = changed_payload(&mut listing);
        let encoded = encode(&payload).unwrap_or_else(|error| panic!("encode: {error}"));

        assert_eq!(
            payload,
            decode(
                ProductListingEventType::Changed.as_str(),
                PRODUCT_LISTING_EVENT_SCHEMA_VERSION,
                &encoded,
            )
            .unwrap_or_else(|error| panic!("decode: {error}")),
        );
    }

    #[test]
    fn should_round_trip_withdrawal_with_previous_availability() {
        let mut listing = sample_listing();
        listing.take_pending_event_payload();
        listing.withdraw();
        let payload = changed_payload(&mut listing);
        let encoded = encode(&payload).unwrap_or_else(|error| panic!("encode: {error}"));

        assert_eq!(
            payload,
            decode(
                ProductListingEventType::Changed.as_str(),
                PRODUCT_LISTING_EVENT_SCHEMA_VERSION,
                &encoded,
            )
            .unwrap_or_else(|error| panic!("decode: {error}")),
        );
    }

    #[test]
    fn should_round_trip_retracted_sale_observation_change() {
        let mut listing = sample_listing();
        listing.take_pending_event_payload();
        let observation =
            ListingSaleObservation::new(OffsetDateTime::UNIX_EPOCH, fxrate_core::FxRateId::new());
        listing
            .record_sale_observation(observation)
            .unwrap_or_else(|error| panic!("sale observation: {error}"));
        listing.take_pending_event_payload();
        listing.retract_sale_observation();
        let payload = changed_payload(&mut listing);
        let encoded = encode(&payload).unwrap_or_else(|error| panic!("encode: {error}"));

        assert_eq!(
            payload,
            decode(
                ProductListingEventType::Changed.as_str(),
                PRODUCT_LISTING_EVENT_SCHEMA_VERSION,
                &encoded,
            )
            .unwrap_or_else(|error| panic!("decode: {error}")),
        );
    }

    #[test]
    fn should_reject_unknown_fields_versions_types_and_empty_changes() {
        let payload = discovered_payload();
        let mut unknown = encode(&payload).unwrap_or_else(|error| panic!("encode: {error}"));
        unknown["kind"] = json!("discovered");

        for (event_type, version, payload) in [
            (
                ProductListingEventType::Discovered.as_str(),
                PRODUCT_LISTING_EVENT_SCHEMA_VERSION,
                unknown,
            ),
            (ProductListingEventType::Discovered.as_str(), 2, json!({})),
            ("PRODUCT_LISTING_UNKNOWN", 1, json!({})),
            (ProductListingEventType::Changed.as_str(), 1, json!({})),
        ] {
            assert!(decode(event_type, version, &payload).is_err());
        }
    }

    fn discovered_payload() -> ProductListingEventPayload {
        let mut listing = sample_listing();
        match listing.take_pending_event_payload() {
            Some(payload) => payload,
            None => panic!("new listing must emit a discovered event"),
        }
    }

    fn changed_payload(listing: &mut ProductListing) -> ProductListingEventPayload {
        match listing.take_pending_event_payload() {
            Some(ProductListingEventPayload::Changed(payload)) => {
                ProductListingEventPayload::Changed(payload)
            }
            _ => panic!("listing must emit a changed event"),
        }
    }

    fn sample_listing() -> ProductListing {
        ProductListing::create(NewProductListing {
            id: ProductListingId::new(),
            title_slug_id: ProductListingSlugId::raw("codec-test-a1b2c3")
                .unwrap_or_else(|error| panic!("slug: {error}")),
            listing_source_id: ListingSourceId::new(),
            source_listing_id: SourceListingId::try_from("codec-source-listing")
                .unwrap_or_else(|error| panic!("source ID: {error}")),
            title: Some(Localized::new(Language::En, Title::from("Codec title"))),
            description: Some(Localized::new(
                Language::En,
                Description::from("Codec description"),
            )),
            pricing: ProductListingPricing {
                price: Some(price(10)),
                price_estimate_min: None,
                price_estimate_max: None,
            },
            availability: Some(ListingAvailability::Available),
            url: parse_url("https://example.com/old")
                .unwrap_or_else(|error| panic!("URL: {error}")),
            images: images(1, "seed").unwrap_or_else(|error| panic!("images: {error}")),
            auction: ProductListingAuction::default(),
        })
        .unwrap_or_else(|error| panic!("listing: {error}"))
    }

    fn price(amount: u64) -> Price {
        Price::new(MonetaryAmount::from(amount), Currency::Eur)
    }
}
