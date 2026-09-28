//! Versioned, source-neutral ProductListing ingestion wire contract.
//!
//! This is a transport codec, not a provider parser or a canonical write. No credentials or
//! transport receipt IDs are used as business identity. Wire changes require a new schema version.

use application::{
    operation_context::{CorrelationId, RequestId},
    patch_field::PatchField,
};
use indexmap::IndexSet;
use listing_source_core::ListingSourceId;
use localization::{Language, Localized};
use money::{Currency, MonetaryAmount, Price};
use product_listing_core::{
    description::Description,
    listing_availability::ListingAvailability,
    product_listing::{CataloguePosition, LotNumber, ProductListingPricing},
    product_listing_id::ProductListingKey,
    product_listing_image::ProductListingImage,
    product_listing_price::ProductListingPrice,
    source_listing_id::SourceListingId,
    title::Title,
};
use product_listing_normalization::{
    NormalizationContext, ProductListingNormalizationInput, RawProductListingOperation,
    RawProductListingPayloadFormat, RawProductListingProvenance, RawProductListingValues,
    SourcePayload,
};
use product_listing_service::{
    ports::{
        ProductListingRawIngestionMethod, ProductListingRawProviderReceipt, ProviderReceiptScope,
        SourceEvidenceSha256,
    },
    product_listing_auction_patch::ProductListingAuctionPatch,
    use_cases::commands::product_listing_ingestion::{
        ProductListingIngestionActor, ProductListingIngestionCommandId,
        ProductListingIngestionFingerprint, ProductListingIngestionIntent,
        ProductListingIngestionMessage, ProductListingIngestionMetadata,
        ProductListingIngestionOperation, ProductListingIngestionSubmissionId,
    },
    use_cases::{
        CaptureProductListingRawObservationCommand, CreateProductListingCommand,
        ProductListingIngestionEnvelope, UpdateProductListingCommand, UpsertProductListingCommand,
    },
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use time::{OffsetDateTime, format_description::well_known::Rfc3339};
use url::Url;

pub const SCHEMA_VERSION: u8 = 1;
const MAX_MESSAGE_BYTES: usize = 1024 * 1024;
const GROUP_DOMAIN: &[u8] = b"aura.product-listing-ingestion.fifo-group.v1";
const DEDUP_DOMAIN: &[u8] = b"aura.product-listing-ingestion.fifo-dedup.v1";
const FINGERPRINT_DOMAIN: &[u8] = b"aura.product-listing-ingestion.semantic.v1";

/// Errors deliberately exclude source payloads, credential data and JSON parser details.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum CodecError {
    #[error("invalid ingestion envelope or command")]
    Invalid,
    #[error("unsupported ingestion schema version")]
    UnsupportedVersion,
    #[error("ingestion message exceeds the transport size limit")]
    TooLarge,
}

type Result<T> = std::result::Result<T, CodecError>;

/// The entire durable v1 message; unknown fields and operation variants are rejected.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct IngestionEnvelopeV1 {
    schema_version: u8,
    submission_id: String,
    command_id: String,
    index: usize,
    input_count: usize,
    listing_source_id: String,
    actor: WireActor,
    request_id: String,
    correlation_id: String,
    prepared_at: String,
    semantic_fingerprint: String,
    payload: WirePayload,
}

impl IngestionEnvelopeV1 {
    pub fn command_id(&self) -> &str {
        &self.command_id
    }
    pub fn submission_id(&self) -> &str {
        &self.submission_id
    }
    pub fn index(&self) -> usize {
        self.index
    }
    pub fn input_count(&self) -> usize {
        self.input_count
    }
    pub fn actor(&self) -> Result<ProductListingIngestionActor> {
        self.validate()?;
        self.actor.to_actor()
    }
    pub fn request_id(&self) -> RequestId {
        RequestId::new(&self.request_id)
    }
    pub fn correlation_id(&self) -> CorrelationId {
        CorrelationId::new(&self.correlation_id)
    }
    pub fn operation(&self) -> ProductListingIngestionOperation {
        self.payload.operation()
    }

    pub fn prepared_at(&self) -> Result<OffsetDateTime> {
        self.validate()?;
        instant(&self.prepared_at)
    }

    /// Returns only a fingerprint verified against the actor scope and typed command.
    /// Never trust the wire field directly: it is not an authentication signature.
    pub fn verified_fingerprint(&self) -> Result<&str> {
        self.validate()?;
        Ok(&self.semantic_fingerprint)
    }

    /// Service-owned typed digest for per-command downstream conflict detection.
    pub fn typed_fingerprint(&self) -> Result<ProductListingIngestionFingerprint> {
        self.validate()?;
        Ok(ProductListingIngestionFingerprint::from_digest(
            parse_digest(&self.semantic_fingerprint)?,
        ))
    }

    /// Maps only command data; the caller must retain the validated envelope's actor and IDs.
    pub fn into_intent(self) -> Result<ProductListingIngestionIntent> {
        self.validate()?;
        self.payload.into_intent()
    }

    /// Revalidate the wire fingerprint and all typed fields before constructing the service input.
    /// Transport IDs never become business identity or authorize an actor on their own.
    pub fn into_service_envelope(self) -> Result<ProductListingIngestionEnvelope> {
        self.validate()?;
        let fingerprint = self.typed_fingerprint()?;
        let operation = self.payload.operation();
        Ok(ProductListingIngestionEnvelope {
            message: ProductListingIngestionMessage {
                metadata: ProductListingIngestionMetadata {
                    submission_id: ProductListingIngestionSubmissionId::from_wire(
                        &self.submission_id,
                    )
                    .ok_or(CodecError::Invalid)?,
                    command_id: ProductListingIngestionCommandId::from_wire(&self.command_id)
                        .ok_or(CodecError::Invalid)?,
                    index: self.index,
                    input_count: self.input_count,
                    listing_source_id: parse_source(&self.listing_source_id)?,
                    operation,
                    actor: self.actor.to_actor()?,
                    request_id: RequestId::new(self.request_id),
                    correlation_id: CorrelationId::new(self.correlation_id),
                },
                intent: self.payload.into_intent()?,
            },
            fingerprint,
        })
    }

    fn validate(&self) -> Result<()> {
        if self.schema_version != SCHEMA_VERSION {
            return Err(CodecError::UnsupportedVersion);
        }
        if self.input_count == 0
            || self.index >= self.input_count
            || !valid_digest_id(&self.submission_id, "plis1_")
            || !valid_digest_id(&self.command_id, "plic1_")
            || self.request_id.is_empty()
            || self.correlation_id.is_empty()
            || self.request_id.contains('\0')
            || self.correlation_id.contains('\0')
        {
            return Err(CodecError::Invalid);
        }
        self.actor.validate()?;
        let source = parse_source(&self.listing_source_id)?;
        if self.payload.listing_source_id()? != source {
            return Err(CodecError::Invalid);
        }
        // Validate every leaf before a publisher is permitted to send this envelope.
        self.payload.clone().into_intent()?;
        let prepared_at = instant(&self.prepared_at)?;
        if prepared_at.offset() != time::UtcOffset::UTC
            || format_instant(prepared_at)? != self.prepared_at
            || !valid_fingerprint(&self.semantic_fingerprint)
            || self.semantic_fingerprint != fingerprint(&self.actor, &self.payload)?
        {
            return Err(CodecError::Invalid);
        }
        Ok(())
    }
}

fn valid_digest_id(value: &str, prefix: &str) -> bool {
    value.strip_prefix(prefix).is_some_and(valid_fingerprint)
}

fn valid_fingerprint(digest: &str) -> bool {
    digest.len() == 64
        && digest
            .bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct WireActor {
    kind: String,
    id: Option<String>,
}

impl WireActor {
    fn from_actor(actor: &ProductListingIngestionActor) -> Self {
        Self {
            kind: actor.principal_kind().to_owned(),
            id: actor.actor_id(),
        }
    }
    // Credential transport kind stays on the wire; both ways of acting as the same user
    // share the service's USER idempotency scope.
    fn identity_kind(&self) -> &str {
        match self.kind.as_str() {
            "DELEGATED_USER" => "USER",
            _ => &self.kind,
        }
    }
    fn to_actor(&self) -> Result<ProductListingIngestionActor> {
        match (self.kind.as_str(), self.id.as_deref()) {
            ("USER", Some(id)) => Ok(ProductListingIngestionActor::User(
                id.parse().map_err(|_| CodecError::Invalid)?,
            )),
            ("DELEGATED_USER", Some(id)) => Ok(ProductListingIngestionActor::DelegatedUser(
                id.parse().map_err(|_| CodecError::Invalid)?,
            )),
            ("SERVICE", Some(id)) if !id.is_empty() && !id.contains('\0') => {
                Ok(ProductListingIngestionActor::Service(id.to_owned()))
            }
            ("SYSTEM", None) => Ok(ProductListingIngestionActor::System),
            _ => Err(CodecError::Invalid),
        }
    }
    fn validate(&self) -> Result<()> {
        match (self.kind.as_str(), self.id.as_deref()) {
            ("USER" | "DELEGATED_USER", Some(id))
                if id.parse::<user_core::user_id::UserId>().is_ok() =>
            {
                Ok(())
            }
            ("SERVICE", Some(id)) if !id.is_empty() && !id.contains('\0') => Ok(()),
            ("SYSTEM", None) => Ok(()),
            _ => Err(CodecError::Invalid),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "operation",
    content = "command",
    rename_all = "SCREAMING_SNAKE_CASE",
    deny_unknown_fields
)]
enum WirePayload {
    Create(WireCreate),
    Update(WireUpdate),
    Upsert(WireUpsert),
    Withdraw(WireKey),
    CaptureRaw(WireRaw),
}

impl WirePayload {
    fn operation(&self) -> ProductListingIngestionOperation {
        match self {
            Self::Create(_) => ProductListingIngestionOperation::Create,
            Self::Update(_) => ProductListingIngestionOperation::Update,
            Self::Upsert(_) => ProductListingIngestionOperation::Upsert,
            Self::Withdraw(_) => ProductListingIngestionOperation::Withdraw,
            Self::CaptureRaw(_) => ProductListingIngestionOperation::CaptureRaw,
        }
    }
    fn listing_source_id(&self) -> Result<ListingSourceId> {
        parse_source(match self {
            Self::Create(c) => &c.key.listing_source_id,
            Self::Update(c) => &c.key.listing_source_id,
            Self::Upsert(c) => &c.key.listing_source_id,
            Self::Withdraw(c) => &c.listing_source_id,
            Self::CaptureRaw(c) => &c.listing_source_id,
        })
    }
    fn into_intent(self) -> Result<ProductListingIngestionIntent> {
        Ok(match self {
            Self::Create(c) => ProductListingIngestionIntent::Create(CreateProductListingCommand {
                listing_source_id: parse_source(&c.key.listing_source_id)?,
                source_listing_id: parse_listing(&c.key.source_listing_id)?,
                title: localized_title(c.title)?,
                description: localized_description(c.description)?,
                pricing: ProductListingPricing {
                    price: c.pricing.price.map(price_assertion).transpose()?,
                    price_estimate_min: c.pricing.price_estimate_min.map(price).transpose()?,
                    price_estimate_max: c.pricing.price_estimate_max.map(price).transpose()?,
                },
                availability: c.availability.as_deref().map(availability).transpose()?,
                url: parse_url(&c.url)?,
                images: images(c.images)?,
                auction: c.auction.map(auction).transpose()?,
            }),
            Self::Update(c) => {
                if matches!(&c.url, WirePatch::Clear) {
                    return Err(CodecError::Invalid);
                }
                ProductListingIngestionIntent::Update {
                    product_key: key(c.key)?,
                    command: UpdateProductListingCommand {
                        price: map_patch(c.price, price_assertion)?,
                        price_estimate_min: map_patch(c.price_estimate_min, price)?,
                        price_estimate_max: map_patch(c.price_estimate_max, price)?,
                        availability: map_patch(c.availability, |s| availability(&s))?,
                        url: map_patch(c.url, |s| parse_url(&s))?,
                        images: map_patch(c.images, images)?,
                        auction: map_patch(c.auction, auction)?,
                    },
                }
            }
            Self::Upsert(c) => ProductListingIngestionIntent::Upsert(UpsertProductListingCommand {
                listing_source_id: parse_source(&c.key.listing_source_id)?,
                source_listing_id: parse_listing(&c.key.source_listing_id)?,
                title: localized_title(c.title)?,
                description: localized_description(c.description)?,
                price: map_patch(c.price, price_assertion)?,
                price_estimate_min: map_patch(c.price_estimate_min, price)?,
                price_estimate_max: map_patch(c.price_estimate_max, price)?,
                availability: map_patch(c.availability, |s| availability(&s))?,
                url: c.url.map(|s| parse_url(&s)).transpose()?,
                images: map_patch(c.images, images)?,
                auction: map_patch(c.auction, auction)?,
            }),
            Self::Withdraw(c) => ProductListingIngestionIntent::Withdraw(key(c)?),
            Self::CaptureRaw(c) => {
                if c.source_record_key.len() > product_listing_service::use_cases::commands::capture_product_listing_raw_observation::MAX_SOURCE_RECORD_KEY_UTF8_BYTES
                    || c.source_record_key.contains('\0')
                    || c.source_event_id
                        .as_deref()
                        .is_some_and(|id| id.contains('\0'))
                {
                    return Err(CodecError::Invalid);
                }
                let receipt = c
                    .provider_receipt
                    .map(|r| {
                        ProductListingRawProviderReceipt::new(
                            ProviderReceiptScope::new(r.scope).map_err(|_| CodecError::Invalid)?,
                            r.delivery_id,
                            SourceEvidenceSha256::new(parse_digest(&r.source_evidence_sha256)?),
                        )
                        .map_err(|_| CodecError::Invalid)
                    })
                    .transpose()?;
                ProductListingIngestionIntent::CaptureRaw(
                    CaptureProductListingRawObservationCommand {
                        listing_source_id: parse_source(&c.listing_source_id)?,
                        ingestion_method: ProductListingRawIngestionMethod::from_code(
                            &c.ingestion_method,
                        )
                        .ok_or(CodecError::Invalid)?,
                        source_record_key: c.source_record_key,
                        input: ProductListingNormalizationInput::new(
                            RawProductListingOperation::from_code(&c.input.operation)
                                .ok_or(CodecError::Invalid)?,
                            RawProductListingPayloadFormat::from_code(&c.input.payload_format)
                                .ok_or(CodecError::Invalid)?,
                            c.input.payload_schema_version,
                            c.input.raw_values_schema_version,
                            SourcePayload::new(c.input.source_payload)
                                .map_err(|_| CodecError::Invalid)?,
                            RawProductListingValues::new(c.input.raw_values)
                                .map_err(|_| CodecError::Invalid)?,
                            NormalizationContext::new(c.input.normalization_context)
                                .map_err(|_| CodecError::Invalid)?,
                        )
                        .map_err(|_| CodecError::Invalid)?,
                        provenance: RawProductListingProvenance::new(c.provenance)
                            .map_err(|_| CodecError::Invalid)?,
                        source_event_id: c.source_event_id,
                        source_occurred_at: c
                            .source_occurred_at
                            .map(|s| instant(&s))
                            .transpose()?,
                        provider_receipt: receipt,
                    },
                )
            }
        })
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct WireKey {
    listing_source_id: String,
    source_listing_id: String,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct WireLocalized {
    language: String,
    value: String,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct WirePrice {
    currency: String,
    amount: u64,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "SCREAMING_SNAKE_CASE", deny_unknown_fields)]
enum WirePriceAssertion {
    Monetary { currency: String, amount: u64 },
    OnRequest,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct WirePricing {
    price: Option<WirePriceAssertion>,
    price_estimate_min: Option<WirePrice>,
    price_estimate_max: Option<WirePrice>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "state",
    content = "value",
    rename_all = "SCREAMING_SNAKE_CASE",
    deny_unknown_fields
)]
enum WirePatch<T> {
    Unchanged,
    Clear,
    Set(T),
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct WireAuction {
    auction_id: WirePatch<String>,
    lot_number: WirePatch<String>,
    catalogue_position: WirePatch<u32>,
    bidding_opens: WirePatch<String>,
    scheduled_closes: WirePatch<String>,
    reported_closed_at: WirePatch<String>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct WireCreate {
    key: WireKey,
    title: Option<WireLocalized>,
    description: Option<WireLocalized>,
    pricing: WirePricing,
    availability: Option<String>,
    url: String,
    images: Vec<String>,
    auction: Option<WireAuction>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct WireUpdate {
    key: WireKey,
    price: WirePatch<WirePriceAssertion>,
    price_estimate_min: WirePatch<WirePrice>,
    price_estimate_max: WirePatch<WirePrice>,
    availability: WirePatch<String>,
    url: WirePatch<String>,
    images: WirePatch<Vec<String>>,
    auction: WirePatch<WireAuction>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct WireUpsert {
    key: WireKey,
    title: Option<WireLocalized>,
    description: Option<WireLocalized>,
    price: WirePatch<WirePriceAssertion>,
    price_estimate_min: WirePatch<WirePrice>,
    price_estimate_max: WirePatch<WirePrice>,
    availability: WirePatch<String>,
    url: Option<String>,
    images: WirePatch<Vec<String>>,
    auction: WirePatch<WireAuction>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct WireInput {
    operation: String,
    payload_format: String,
    payload_schema_version: u16,
    raw_values_schema_version: u16,
    source_payload: Value,
    raw_values: Value,
    normalization_context: Value,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct WireReceipt {
    scope: String,
    delivery_id: String,
    source_evidence_sha256: String,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct WireRaw {
    listing_source_id: String,
    ingestion_method: String,
    source_record_key: String,
    input: WireInput,
    provenance: Value,
    source_event_id: Option<String>,
    source_occurred_at: Option<String>,
    provider_receipt: Option<WireReceipt>,
}

fn parse_source(s: &str) -> Result<ListingSourceId> {
    let source: ListingSourceId = s.parse().map_err(|_| CodecError::Invalid)?;
    if source.to_string() != s {
        return Err(CodecError::Invalid);
    }
    Ok(source)
}
fn parse_listing(s: &str) -> Result<SourceListingId> {
    let id = SourceListingId::try_from(s).map_err(|_| CodecError::Invalid)?;
    if id.as_ref() != s {
        return Err(CodecError::Invalid);
    }
    Ok(id)
}
fn key(k: WireKey) -> Result<ProductListingKey> {
    Ok(ProductListingKey::new(
        parse_source(&k.listing_source_id)?,
        parse_listing(&k.source_listing_id)?,
    ))
}
fn parse_url(s: &str) -> Result<Url> {
    Url::parse(s).map_err(|_| CodecError::Invalid)
}
fn instant(s: &str) -> Result<OffsetDateTime> {
    OffsetDateTime::parse(s, &Rfc3339).map_err(|_| CodecError::Invalid)
}
fn format_instant(t: OffsetDateTime) -> Result<String> {
    t.format(&Rfc3339).map_err(|_| CodecError::Invalid)
}
fn availability(s: &str) -> Result<ListingAvailability> {
    ListingAvailability::from_code(s).ok_or(CodecError::Invalid)
}
fn price(p: WirePrice) -> Result<Price> {
    Ok(Price::new(
        MonetaryAmount::from(p.amount),
        Currency::from_code(&p.currency).ok_or(CodecError::Invalid)?,
    ))
}
fn price_assertion(p: WirePriceAssertion) -> Result<ProductListingPrice> {
    match p {
        WirePriceAssertion::Monetary { currency, amount } => {
            Ok(ProductListingPrice::Monetary(price(WirePrice {
                currency,
                amount,
            })?))
        }
        WirePriceAssertion::OnRequest => Ok(ProductListingPrice::OnRequest),
    }
}
fn localized_title(l: Option<WireLocalized>) -> Result<Option<Localized<Language, Title>>> {
    l.map(|l| {
        let title = Title::from(l.value.as_str());
        if title.as_ref() != l.value {
            return Err(CodecError::Invalid);
        }
        Ok(Localized::new(
            Language::from_code(&l.language).ok_or(CodecError::Invalid)?,
            title,
        ))
    })
    .transpose()
}
fn localized_description(
    l: Option<WireLocalized>,
) -> Result<Option<Localized<Language, Description>>> {
    l.map(|l| {
        let description = Description::from(l.value.as_str());
        if description.as_ref() != l.value {
            return Err(CodecError::Invalid);
        }
        Ok(Localized::new(
            Language::from_code(&l.language).ok_or(CodecError::Invalid)?,
            description,
        ))
    })
    .transpose()
}
fn images(values: Vec<String>) -> Result<IndexSet<ProductListingImage>> {
    let count = values.len();
    let images = values
        .into_iter()
        .map(|s| parse_url(&s).map(ProductListingImage::new))
        .collect::<Result<IndexSet<_>>>()?;
    if count != images.len() {
        return Err(CodecError::Invalid);
    }
    Ok(images)
}
fn map_patch<T, U>(patch: WirePatch<T>, f: impl FnOnce(T) -> Result<U>) -> Result<PatchField<U>> {
    Ok(match patch {
        WirePatch::Unchanged => PatchField::Unchanged,
        WirePatch::Clear => PatchField::Clear,
        WirePatch::Set(value) => PatchField::Set(f(value)?),
    })
}
fn auction(a: WireAuction) -> Result<ProductListingAuctionPatch> {
    let patch = ProductListingAuctionPatch {
        auction_id: map_patch(a.auction_id, |s| s.parse().map_err(|_| CodecError::Invalid))?,
        lot_number: map_patch(a.lot_number, |s| {
            LotNumber::parse(&s).map_err(|_| CodecError::Invalid)
        })?,
        catalogue_position: map_patch(a.catalogue_position, |n| {
            CataloguePosition::new(n).map_err(|_| CodecError::Invalid)
        })?,
        bidding_opens: map_patch(a.bidding_opens, |s| instant(&s))?,
        scheduled_closes: map_patch(a.scheduled_closes, |s| instant(&s))?,
        reported_closed_at: map_patch(a.reported_closed_at, |s| instant(&s))?,
    };
    product_listing_service::product_listing_auction_patch::validate_product_listing_auction_patch(
        None, &patch,
    )
    .map_err(|_| CodecError::Invalid)?;
    Ok(patch)
}
fn parse_digest(s: &str) -> Result<[u8; 32]> {
    if s.len() != 64
        || !s
            .bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
    {
        return Err(CodecError::Invalid);
    }
    let mut out = [0; 32];
    for (i, byte) in out.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&s[i * 2..i * 2 + 2], 16).map_err(|_| CodecError::Invalid)?;
    }
    Ok(out)
}
fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
fn fingerprint(actor: &WireActor, payload: &WirePayload) -> Result<String> {
    actor.validate()?;
    payload.clone().into_intent()?;
    let bytes = serde_json::to_vec(payload).map_err(|_| CodecError::Invalid)?;
    Ok(digest(
        FINGERPRINT_DOMAIN,
        &[
            actor.identity_kind().as_bytes(),
            actor.id.as_deref().unwrap_or("").as_bytes(),
            &bytes,
        ],
    ))
}

fn digest(domain: &[u8], fields: &[&[u8]]) -> String {
    let mut h = Sha256::new();
    for field in std::iter::once(domain).chain(fields.iter().copied()) {
        h.update((field.len() as u64).to_be_bytes());
        h.update(field);
    }
    hex(&h.finalize())
}

fn wire_price(p: Price) -> WirePrice {
    WirePrice {
        currency: p.currency.as_str().into(),
        amount: *p.monetary_amount,
    }
}
fn wire_assertion(p: ProductListingPrice) -> WirePriceAssertion {
    match p {
        ProductListingPrice::Monetary(p) => WirePriceAssertion::Monetary {
            currency: p.currency.as_str().into(),
            amount: *p.monetary_amount,
        },
        ProductListingPrice::OnRequest => WirePriceAssertion::OnRequest,
    }
}
fn wire_patch<T, U>(p: &PatchField<T>, f: impl FnOnce(&T) -> U) -> WirePatch<U> {
    match p {
        PatchField::Unchanged => WirePatch::Unchanged,
        PatchField::Clear => WirePatch::Clear,
        PatchField::Set(v) => WirePatch::Set(f(v)),
    }
}
fn wire_auction(a: &ProductListingAuctionPatch) -> Result<WireAuction> {
    fn timestamp(p: &PatchField<OffsetDateTime>) -> Result<WirePatch<String>> {
        match p {
            PatchField::Set(t) => Ok(WirePatch::Set(format_instant(*t)?)),
            PatchField::Clear => Ok(WirePatch::Clear),
            PatchField::Unchanged => Ok(WirePatch::Unchanged),
        }
    }
    Ok(WireAuction {
        auction_id: wire_patch(&a.auction_id, ToString::to_string),
        lot_number: wire_patch(&a.lot_number, |v| v.as_str().to_owned()),
        catalogue_position: wire_patch(&a.catalogue_position, |v| v.value()),
        bidding_opens: timestamp(&a.bidding_opens)?,
        scheduled_closes: timestamp(&a.scheduled_closes)?,
        reported_closed_at: timestamp(&a.reported_closed_at)?,
    })
}
fn wire_key(source: ListingSourceId, listing: &SourceListingId) -> WireKey {
    WireKey {
        listing_source_id: source.to_string(),
        source_listing_id: listing.to_string(),
    }
}
fn wire_images(images: &IndexSet<ProductListingImage>) -> Vec<String> {
    images.iter().map(|i| i.url().to_string()).collect()
}
fn wire_localized<T: AsRef<str>>(value: &Option<Localized<Language, T>>) -> Option<WireLocalized> {
    value.as_ref().map(|v| WireLocalized {
        language: v.localization.as_str().into(),
        value: v.payload.as_ref().into(),
    })
}
fn wire_payload(intent: &ProductListingIngestionIntent) -> Result<WirePayload> {
    Ok(match intent {
        ProductListingIngestionIntent::Create(c) => WirePayload::Create(WireCreate {
            key: wire_key(c.listing_source_id, &c.source_listing_id),
            title: wire_localized(&c.title),
            description: wire_localized(&c.description),
            pricing: WirePricing {
                price: c.pricing.price.map(wire_assertion),
                price_estimate_min: c.pricing.price_estimate_min.map(wire_price),
                price_estimate_max: c.pricing.price_estimate_max.map(wire_price),
            },
            availability: c.availability.map(|v| v.as_str().into()),
            url: c.url.to_string(),
            images: wire_images(&c.images),
            auction: c.auction.as_ref().map(wire_auction).transpose()?,
        }),
        ProductListingIngestionIntent::Update {
            product_key,
            command: c,
        } => WirePayload::Update(WireUpdate {
            key: wire_key(
                product_key.listing_source_id,
                &product_key.source_listing_id,
            ),
            price: wire_patch(&c.price, |v| wire_assertion(*v)),
            price_estimate_min: wire_patch(&c.price_estimate_min, |v| wire_price(*v)),
            price_estimate_max: wire_patch(&c.price_estimate_max, |v| wire_price(*v)),
            availability: wire_patch(&c.availability, |v| v.as_str().into()),
            url: wire_patch(&c.url, ToString::to_string),
            images: wire_patch(&c.images, wire_images),
            auction: match &c.auction {
                PatchField::Set(v) => WirePatch::Set(wire_auction(v)?),
                PatchField::Clear => WirePatch::Clear,
                PatchField::Unchanged => WirePatch::Unchanged,
            },
        }),
        ProductListingIngestionIntent::Upsert(c) => WirePayload::Upsert(WireUpsert {
            key: wire_key(c.listing_source_id, &c.source_listing_id),
            title: wire_localized(&c.title),
            description: wire_localized(&c.description),
            price: wire_patch(&c.price, |v| wire_assertion(*v)),
            price_estimate_min: wire_patch(&c.price_estimate_min, |v| wire_price(*v)),
            price_estimate_max: wire_patch(&c.price_estimate_max, |v| wire_price(*v)),
            availability: wire_patch(&c.availability, |v| v.as_str().into()),
            url: c.url.as_ref().map(ToString::to_string),
            images: wire_patch(&c.images, wire_images),
            auction: match &c.auction {
                PatchField::Set(v) => WirePatch::Set(wire_auction(v)?),
                PatchField::Clear => WirePatch::Clear,
                PatchField::Unchanged => WirePatch::Unchanged,
            },
        }),
        ProductListingIngestionIntent::Withdraw(k) => {
            WirePayload::Withdraw(wire_key(k.listing_source_id, &k.source_listing_id))
        }
        ProductListingIngestionIntent::CaptureRaw(c) => WirePayload::CaptureRaw(WireRaw {
            listing_source_id: c.listing_source_id.to_string(),
            ingestion_method: c.ingestion_method.as_str().into(),
            source_record_key: c.source_record_key.clone(),
            input: WireInput {
                operation: c.input.operation().as_str().into(),
                payload_format: c.input.payload_format().as_str().into(),
                payload_schema_version: c.input.payload_schema_version(),
                raw_values_schema_version: c.input.raw_values_schema_version(),
                source_payload: c.input.source_payload().value().clone(),
                raw_values: c.input.raw_values().value().clone(),
                normalization_context: c.input.normalization_context().value().clone(),
            },
            provenance: c.provenance.value().clone(),
            source_event_id: c.source_event_id.clone(),
            source_occurred_at: c.source_occurred_at.map(format_instant).transpose()?,
            provider_receipt: c.provider_receipt.as_ref().map(|r| WireReceipt {
                scope: r.scope().as_str().into(),
                delivery_id: r.delivery_id().into(),
                source_evidence_sha256: hex(r.source_evidence_sha256().as_bytes()),
            }),
        }),
    })
}

/// Constructs the strict v1 envelope from a service-prepared message, checking metadata/intent consistency.
pub fn envelope(message: &ProductListingIngestionMessage) -> Result<IngestionEnvelopeV1> {
    let m = &message.metadata;
    let mut value = IngestionEnvelopeV1 {
        schema_version: SCHEMA_VERSION,
        submission_id: m.submission_id.as_str().into(),
        command_id: m.command_id.as_str().into(),
        index: m.index,
        input_count: m.input_count,
        listing_source_id: m.listing_source_id.to_string(),
        actor: WireActor::from_actor(&m.actor),
        request_id: m.request_id.as_str().into(),
        correlation_id: m.correlation_id.as_str().into(),
        prepared_at: format_instant(OffsetDateTime::now_utc())?,
        semantic_fingerprint: String::new(),
        payload: wire_payload(&message.intent)?,
    };
    if m.operation != message.intent.operation() {
        return Err(CodecError::Invalid);
    }
    value.semantic_fingerprint = fingerprint(&value.actor, &value.payload)?;
    value.validate()?;
    Ok(value)
}

/// Encodes an already prepared service message. No lossy fallback or debug serialization.
pub fn encode(message: &ProductListingIngestionMessage) -> Result<String> {
    let value = serde_json::to_string(&envelope(message)?).map_err(|_| CodecError::Invalid)?;
    if value.len() > MAX_MESSAGE_BYTES {
        return Err(CodecError::TooLarge);
    }
    Ok(value)
}

/// Rejects unknown fields, unsupported versions, mismatched source IDs and invalid typed leaves.
pub fn decode(body: &str) -> Result<IngestionEnvelopeV1> {
    if body.len() > MAX_MESSAGE_BYTES {
        return Err(CodecError::TooLarge);
    }
    let value: IngestionEnvelopeV1 = serde_json::from_str(body).map_err(|_| CodecError::Invalid)?;
    value.validate()?;
    Ok(value)
}

/// SHA-256 of actor scope plus versioned canonical semantic command data. Excludes command,
/// submission, request/correlation IDs and prepared time. Object keys are canonicalized;
/// image order, patch states, and raw evidence/provenance are retained.
pub fn semantic_fingerprint(value: &IngestionEnvelopeV1) -> Result<String> {
    Ok(value.verified_fingerprint()?.to_owned())
}

/// Stable per-listing FIFO partition; raw observations partition by method and raw source key.
/// Hashing keeps opaque keys out of SQS message attributes and within the 128-byte FIFO limit.
pub fn fifo_group_id(value: &IngestionEnvelopeV1) -> Result<String> {
    value.validate()?;
    let source = value.listing_source_id.as_bytes();
    let fields: Vec<&[u8]> = match &value.payload {
        WirePayload::Create(c) => vec![source, b"listing", c.key.source_listing_id.as_bytes()],
        WirePayload::Update(c) => vec![source, b"listing", c.key.source_listing_id.as_bytes()],
        WirePayload::Upsert(c) => vec![source, b"listing", c.key.source_listing_id.as_bytes()],
        WirePayload::Withdraw(c) => vec![source, b"listing", c.source_listing_id.as_bytes()],
        WirePayload::CaptureRaw(c) => vec![
            source,
            b"raw",
            c.ingestion_method.as_bytes(),
            c.source_record_key.as_bytes(),
        ],
    };
    Ok(format!("pli1_{}", digest(GROUP_DOMAIN, &fields)))
}

/// Command identity plus semantic fingerprint, not SQS's content-based deduplication (which
/// would change when request/correlation IDs change). Different commands on the same key survive.
pub fn fifo_deduplication_id(value: &IngestionEnvelopeV1) -> Result<String> {
    value.validate()?;
    let fingerprint = value.verified_fingerprint()?;
    Ok(format!(
        "plid1_{}",
        digest(
            DEDUP_DOMAIN,
            &[value.command_id.as_bytes(), fingerprint.as_bytes()]
        )
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};

    const GOLDEN_V1: &str = include_str!("../tests/fixtures/ingestion_v1.json");
    const GOLDEN_SOURCE: &str = "ls_01h455vb4pex5vy7enb1p677vn";
    const GOLDEN_LISTING: &str = "陶器 🏺 / SKU-42";

    fn golden_fixtures() -> Vec<Value> {
        serde_json::from_str(GOLDEN_V1).unwrap()
    }

    #[test]
    fn golden_v1_envelopes_decode_and_reencode_without_losing_fields() {
        let fixtures = golden_fixtures();
        assert_eq!(fixtures.len(), 5);
        for (index, (expected_operation, original)) in
            ["CREATE", "UPDATE", "UPSERT", "WITHDRAW", "CAPTURE_RAW"]
                .into_iter()
                .zip(fixtures)
                .enumerate()
        {
            let envelope = decode(&original.to_string()).unwrap();
            assert_eq!(envelope.operation().as_str(), expected_operation);
            assert_eq!(envelope.index(), index);
            assert_eq!(envelope.input_count(), 5);
            assert_eq!(envelope.listing_source_id, GOLDEN_SOURCE);
            assert_eq!(
                envelope.verified_fingerprint().unwrap(),
                original["semanticFingerprint"].as_str().unwrap()
            );
            assert_eq!(
                envelope.typed_fingerprint().unwrap().as_bytes(),
                &parse_digest(envelope.verified_fingerprint().unwrap()).unwrap()
            );
            // The checked-in fingerprint is a golden value; do not repair it at test time.
            let encoded = serde_json::to_string(&envelope).unwrap();
            assert_eq!(serde_json::from_str::<Value>(&encoded).unwrap(), original);
            assert_eq!(decode(&encoded).unwrap(), envelope);
            let intent = envelope.clone().into_intent().unwrap();
            let service = envelope.clone().into_service_envelope().unwrap();
            assert_eq!(service.message.intent, intent);
            assert_eq!(
                service.message.metadata.command_id.as_str(),
                envelope.command_id()
            );
            assert_eq!(
                service.message.metadata.submission_id.as_str(),
                envelope.submission_id()
            );
            assert_eq!(service.message.metadata.actor, envelope.actor().unwrap());
            assert_eq!(service.message.metadata.index, envelope.index());
            assert_eq!(service.message.metadata.input_count, envelope.input_count());
            assert_eq!(service.fingerprint, envelope.typed_fingerprint().unwrap());
            assert_eq!(wire_payload(&intent).unwrap(), envelope.payload);
            assert_eq!(intent.listing_source_id().to_string(), GOLDEN_SOURCE);
            if let Some(id) = intent.source_listing_id() {
                assert_eq!(id.as_ref(), GOLDEN_LISTING);
            }
        }
        assert_eq!(
            GOLDEN_SOURCE
                .parse::<ListingSourceId>()
                .unwrap()
                .as_uuid()
                .to_string(),
            "01890a5d-ac96-774b-bf1d-d5586c639f75"
        );
    }

    #[tokio::test]
    async fn production_encode_round_trips_all_five_golden_intents() {
        use application::operation_context::{OperationContext, Principal};
        use product_listing_service::{
            ports::{ProductListingIngestionPublishError, ProductListingIngestionPublisher},
            use_cases::{
                IndexedProductListingIngestionIntent, ProductListingIngestionIdempotencyKey,
                ProductListingIngestionItemOutcome, ProductListingIngestionMessage,
                ProductListingIngestionOutcome, ProductListingIngestionSubmission,
                SubmitInternalProductListingIngestionHandler,
                SubmitInternalProductListingIngestionUseCase,
            },
        };
        use std::sync::{Arc, Mutex};

        #[derive(Default)]
        struct Capture(Arc<Mutex<Vec<ProductListingIngestionMessage>>>);
        #[async_trait::async_trait]
        impl ProductListingIngestionPublisher for Capture {
            async fn publish(
                &self,
                commands: Vec<ProductListingIngestionMessage>,
            ) -> std::result::Result<
                Vec<ProductListingIngestionItemOutcome>,
                ProductListingIngestionPublishError,
            > {
                let outcomes = commands
                    .iter()
                    .map(|command| ProductListingIngestionItemOutcome {
                        index: command.metadata.index,
                        command_id: command.metadata.command_id.clone(),
                        outcome: ProductListingIngestionOutcome::Accepted,
                    })
                    .collect();
                self.0.lock().unwrap().extend(commands);
                Ok(outcomes)
            }
        }

        let golden = golden_fixtures();
        let intents = golden
            .iter()
            .map(|value| {
                let envelope = decode(&value.to_string()).unwrap();
                IndexedProductListingIngestionIntent {
                    index: envelope.index(),
                    intent: envelope.into_intent().unwrap(),
                }
            })
            .collect();
        let captured = Arc::new(Mutex::new(Vec::new()));
        let handler = SubmitInternalProductListingIngestionHandler::new(Capture(captured.clone()));
        let result = handler
            .execute(
                &OperationContext {
                    principal: Principal::Service("fixture-intake".into()),
                    request_id: RequestId::new("fixture-request"),
                    correlation_id: CorrelationId::new("fixture-correlation"),
                },
                ProductListingIngestionSubmission {
                    listing_source_id: GOLDEN_SOURCE.parse().unwrap(),
                    original_input_count: golden.len(),
                    idempotency_key: Some(
                        ProductListingIngestionIdempotencyKey::new("golden-key").unwrap(),
                    ),
                    items: intents,
                },
            )
            .await
            .unwrap();
        assert_eq!(result.confirmed_accepted_count(), golden.len());
        let captured = captured.lock().unwrap();
        assert_eq!(captured.len(), golden.len());
        for (message, value) in captured.iter().zip(&golden) {
            let encoded = encode(message).unwrap();
            let decoded = decode(&encoded).unwrap();
            assert_eq!(decoded.clone().into_intent().unwrap(), message.intent);
            assert_eq!(
                decoded.payload,
                serde_json::from_value(value["payload"].clone()).unwrap()
            );
            assert_eq!(
                decoded.verified_fingerprint().unwrap(),
                value["semanticFingerprint"].as_str().unwrap()
            );
            assert_eq!(decoded.actor().unwrap(), message.metadata.actor);
            assert_eq!(
                decoded.clone().into_service_envelope().unwrap().message,
                *message
            );
            assert!(decoded.prepared_at().unwrap() <= OffsetDateTime::now_utc());
        }
    }

    #[test]
    fn service_rehydration_rejects_tampered_identity_or_fingerprint() {
        let original = golden_fixtures().remove(0);
        for (field, replacement) in [
            ("commandId", json!(format!("plic1_{}", "F".repeat(64)))),
            ("submissionId", json!(format!("plis1_{}", "g".repeat(64)))),
            ("semanticFingerprint", json!("0".repeat(64))),
        ] {
            let mut value = original.clone();
            value[field] = replacement;
            assert!(decode(&value.to_string()).is_err());
        }
    }

    fn fixture(operation: &str, command: Value) -> Value {
        json!({
            "schemaVersion": 1,
            "submissionId": format!("plis1_{}", "a".repeat(64)),
            "commandId": format!("plic1_{}", "b".repeat(64)),
            "index": 0, "inputCount": 2,
            "listingSourceId": ListingSourceId::new().to_string(),
            "actor": { "kind": "SYSTEM", "id": null },
            "requestId": "request-1", "correlationId": "correlation-1",
            "preparedAt": "2026-01-01T00:00:00Z",
            "payload": { "operation": operation, "command": command }
        })
    }
    fn key(source: &str) -> Value {
        json!({ "listingSourceId": source, "sourceListingId": " SKU #1 " .trim() })
    }
    // Test fixtures are assembled before they have a typed payload; construct their checksum
    // only once the payload has been filled in. Production decode never supplies missing fields.
    fn fingerprinted_value(value: &Value) -> Result<Value> {
        let mut value = value.clone();
        let actor: WireActor =
            serde_json::from_value(value["actor"].clone()).map_err(|_| CodecError::Invalid)?;
        let payload: WirePayload =
            serde_json::from_value(value["payload"].clone()).map_err(|_| CodecError::Invalid)?;
        value["semanticFingerprint"] = json!(fingerprint(&actor, &payload)?);
        Ok(value)
    }
    fn decode_value(value: &Value) -> Result<IngestionEnvelopeV1> {
        decode(&fingerprinted_value(value)?.to_string())
    }

    // For malformed *semantic* leaves, compute the framing hash without invoking the
    // production validator. A failure then proves the leaf is rejected, not just a stale hash.
    fn assert_semantically_invalid(mut value: Value) {
        let envelope: IngestionEnvelopeV1 = serde_json::from_value(value.clone()).unwrap();
        let payload = serde_json::to_vec(&envelope.payload).unwrap();
        value["semanticFingerprint"] = json!(digest(
            FINGERPRINT_DOMAIN,
            &[
                envelope.actor.identity_kind().as_bytes(),
                envelope.actor.id.as_deref().unwrap_or("").as_bytes(),
                &payload,
            ]
        ));
        assert_eq!(decode(&value.to_string()), Err(CodecError::Invalid));
    }
    fn patch(state: &str) -> Value {
        json!({ "state": state })
    }
    fn create_command(source: &str) -> Value {
        json!({
            "key": key(source), "title": { "language": "en", "value": "Title" },
            "description": null, "pricing": { "price": { "kind": "ON_REQUEST" },
                "priceEstimateMin": null, "priceEstimateMax": null },
            "availability": "IN_STOCK", "url": "https://example.test/listing",
            "images": ["https://example.test/a.jpg", "https://example.test/b.jpg"], "auction": null
        })
    }
    fn update_command(source: &str) -> Value {
        json!({ "key": key(source), "price": { "state": "SET", "value": { "kind": "MONETARY", "currency": "EUR", "amount": 1200 } },
            "priceEstimateMin": patch("UNCHANGED"), "priceEstimateMax": patch("CLEAR"),
            "availability": patch("CLEAR"), "url": patch("UNCHANGED"),
            "images": { "state": "SET", "value": [] }, "auction": patch("UNCHANGED") })
    }
    fn upsert_command(source: &str) -> Value {
        let mut value = update_command(source);
        value["title"] = Value::Null;
        value["description"] = Value::Null;
        value["url"] = Value::Null;
        value
    }
    fn raw_command(source: &str) -> Value {
        json!({ "listingSourceId": source, "ingestionMethod": "SHOPIFY", "sourceRecordKey": "id:1",
            "input": { "operation": "UPSERT", "payloadFormat": "SHOPIFY_PRODUCT", "payloadSchemaVersion": 1,
                "rawValuesSchemaVersion": 1, "sourcePayload": { "b": 2, "a": 1 },
                "rawValues": { "priceFormat": "MACHINE_DECIMAL" }, "normalizationContext": {} },
            "provenance": { "receivedFrom": "provider" }, "sourceEventId": "event-1",
            "sourceOccurredAt": "2026-01-01T00:00:00Z",
            "providerReceipt": { "scope": "shopify-webhook", "deliveryId": "delivery-1",
                "sourceEvidenceSha256": "c".repeat(64) }
        })
    }

    #[test]
    fn golden_intents_preserve_patch_states_auction_leaves_images_and_raw_evidence() {
        let fixtures = golden_fixtures();
        let intents: Vec<_> = fixtures
            .iter()
            .map(|fixture| decode(&fixture.to_string()).unwrap().into_intent().unwrap())
            .collect();
        let ProductListingIngestionIntent::Create(create) = &intents[0] else {
            panic!("expected create")
        };
        assert_eq!(create.source_listing_id.as_ref(), GOLDEN_LISTING);
        assert_eq!(create.title.as_ref().unwrap().localization.as_str(), "en");
        assert_eq!(
            create.description.as_ref().unwrap().payload.as_ref(),
            "Vase émaillé"
        );
        assert_eq!(create.pricing.price, Some(ProductListingPrice::OnRequest));
        assert_eq!(
            create.pricing.price_estimate_min.unwrap().monetary_amount,
            MonetaryAmount::from(1200_u64)
        );
        assert_eq!(
            create
                .images
                .iter()
                .map(|image| image.url().as_str())
                .collect::<Vec<_>>(),
            [
                "https://example.test/image/first.jpg",
                "https://example.test/image/second.jpg"
            ]
        );
        let auction = create.auction.as_ref().unwrap();
        assert!(
            matches!(&auction.auction_id, PatchField::Set(id) if id.to_string() == "auc_01h455vb4pex5vy7enb1p677vn")
        );
        assert!(matches!(&auction.lot_number, PatchField::Set(lot) if lot.as_str() == "Lot 7A"));
        assert!(
            matches!(auction.catalogue_position, PatchField::Set(position) if position.value() == 7)
        );
        assert!(
            matches!(auction.bidding_opens, PatchField::Set(at) if at == instant("2026-02-01T09:00:00Z").unwrap())
        );
        assert!(
            matches!(auction.scheduled_closes, PatchField::Set(at) if at == instant("2026-02-02T09:00:00Z").unwrap())
        );
        assert!(matches!(auction.reported_closed_at, PatchField::Unchanged));

        let ProductListingIngestionIntent::Update {
            product_key,
            command: update,
        } = &intents[1]
        else {
            panic!("expected update")
        };
        assert_eq!(product_key.source_listing_id.as_ref(), GOLDEN_LISTING);
        assert!(
            matches!(update.price, PatchField::Set(ProductListingPrice::Monetary(p)) if p.currency == Currency::Usd && *p.monetary_amount == 2525)
        );
        assert!(matches!(update.price_estimate_min, PatchField::Clear));
        assert!(matches!(update.price_estimate_max, PatchField::Unchanged));
        assert!(matches!(update.availability, PatchField::Clear));
        assert!(matches!(update.url, PatchField::Unchanged));
        assert!(
            matches!(&update.images, PatchField::Set(images) if images.iter().map(|i| i.url().as_str()).collect::<Vec<_>>() ==
            ["https://example.test/image/second.jpg", "https://example.test/image/first.jpg"])
        );
        let PatchField::Set(auction) = &update.auction else {
            panic!("expected auction patch")
        };
        assert!(matches!(auction.auction_id, PatchField::Unchanged));
        assert!(matches!(auction.lot_number, PatchField::Clear));
        assert!(
            matches!(auction.catalogue_position, PatchField::Set(position) if position.value() == 12)
        );
        assert!(matches!(auction.bidding_opens, PatchField::Unchanged));
        assert!(matches!(auction.scheduled_closes, PatchField::Clear));
        assert!(
            matches!(auction.reported_closed_at, PatchField::Set(at) if at == instant("2026-02-03T10:00:00Z").unwrap())
        );

        let ProductListingIngestionIntent::Upsert(upsert) = &intents[2] else {
            panic!("expected upsert")
        };
        assert!(matches!(upsert.price, PatchField::Unchanged));
        assert!(
            matches!(upsert.price_estimate_min, PatchField::Set(p) if p.currency == Currency::Jpy && *p.monetary_amount == 1200)
        );
        assert!(matches!(upsert.price_estimate_max, PatchField::Clear));
        assert!(matches!(
            upsert.availability,
            PatchField::Set(ListingAvailability::SoldOut)
        ));
        assert!(matches!(upsert.images, PatchField::Clear));
        assert!(matches!(upsert.auction, PatchField::Unchanged));
        assert_eq!(
            upsert.url.as_ref().unwrap().as_str(),
            "https://example.test/ceramics/42-relisted"
        );
        let ProductListingIngestionIntent::Withdraw(withdraw) = &intents[3] else {
            panic!("expected withdraw")
        };
        assert_eq!(withdraw.source_listing_id.as_ref(), GOLDEN_LISTING);

        let ProductListingIngestionIntent::CaptureRaw(raw) = &intents[4] else {
            panic!("expected raw")
        };
        assert_eq!(raw.source_record_key, GOLDEN_LISTING);
        assert_eq!(
            raw.ingestion_method,
            ProductListingRawIngestionMethod::Shopify
        );
        assert_eq!(
            raw.input.source_payload().value(),
            &fixtures[4]["payload"]["command"]["input"]["sourcePayload"]
        );
        assert_eq!(
            raw.input.raw_values().value(),
            &fixtures[4]["payload"]["command"]["input"]["rawValues"]
        );
        assert_eq!(
            raw.input.normalization_context().value(),
            &fixtures[4]["payload"]["command"]["input"]["normalizationContext"]
        );
        assert_eq!(
            raw.provenance.value(),
            &fixtures[4]["payload"]["command"]["provenance"]
        );
        assert_eq!(raw.source_event_id.as_deref(), Some("provider-event-42"));
        assert_eq!(
            raw.source_occurred_at,
            Some(instant("2026-01-01T01:23:45Z").unwrap())
        );
        let receipt = raw.provider_receipt.as_ref().unwrap();
        assert_eq!(receipt.scope().as_str(), "shopify-webhook");
        assert_eq!(receipt.delivery_id(), "webhook-42");
        assert_eq!(receipt.source_evidence_sha256().as_bytes(), &[0xcc; 32]);
    }

    #[test]
    fn update_patch_state_variants_round_trip_as_distinct_instructions() {
        let update = golden_fixtures()[1].clone();
        let baseline = decode(&update.to_string()).unwrap();
        for (field, replacement) in [
            ("price", json!({"state": "CLEAR"})),
            (
                "priceEstimateMin",
                json!({"state": "SET", "value": {"currency": "EUR", "amount": 0}}),
            ),
            (
                "priceEstimateMax",
                json!({"state": "SET", "value": {"currency": "USD", "amount": 999}}),
            ),
            ("availability", json!({"state": "UNCHANGED"})),
            (
                "url",
                json!({"state": "SET", "value": "https://example.test/new"}),
            ),
            ("images", json!({"state": "CLEAR"})),
            ("auction", json!({"state": "UNCHANGED"})),
        ] {
            let mut changed = update.clone();
            changed["payload"]["command"][field] = replacement;
            let changed = decode(&fingerprinted_value(&changed).unwrap().to_string()).unwrap();
            let expected_payload = changed.payload.clone();
            let intent = changed.clone().into_intent().unwrap();
            assert_eq!(wire_payload(&intent).unwrap(), expected_payload, "{field}");
            assert_ne!(
                semantic_fingerprint(&baseline),
                semantic_fingerprint(&changed),
                "{field}"
            );
        }
    }

    #[test]
    fn malformed_golden_fields_and_tampered_fingerprints_fail_closed() {
        let fixtures = golden_fixtures();
        for original in &fixtures {
            let mut tampered = original.clone();
            tampered["semanticFingerprint"] = json!("0".repeat(64));
            assert!(decode(&tampered.to_string()).is_err());
            tampered = original.clone();
            tampered["schemaVersion"] = json!(2);
            assert_eq!(
                decode(&tampered.to_string()),
                Err(CodecError::UnsupportedVersion)
            );
            tampered = original.clone();
            tampered["unexpected"] = json!(true);
            assert!(decode(&tampered.to_string()).is_err());
        }
        for field in [
            "price",
            "priceEstimateMin",
            "priceEstimateMax",
            "availability",
            "url",
            "images",
            "auction",
        ] {
            let mut missing = fixtures[1].clone();
            missing["payload"]["command"]
                .as_object_mut()
                .unwrap()
                .remove(field);
            assert!(
                serde_json::from_value::<IngestionEnvelopeV1>(missing.clone()).is_err(),
                "missing {field}"
            );
            assert!(decode(&missing.to_string()).is_err());
        }
        for malformed in [
            json!(null),
            json!({"state": "SET"}),
            json!({"state": "SET", "value": null}),
            json!({"state": "BOGUS"}),
        ] {
            let mut bad = fixtures[1].clone();
            bad["payload"]["command"]["price"] = malformed;
            assert!(serde_json::from_value::<IngestionEnvelopeV1>(bad.clone()).is_err());
            assert!(decode(&bad.to_string()).is_err());
        }
        let mut unknown_patch_field = fixtures[1].clone();
        unknown_patch_field["payload"]["command"]["price"]["extra"] = json!(true);
        assert!(serde_json::from_value::<IngestionEnvelopeV1>(unknown_patch_field).is_err());
        let mut bad = fixtures[0].clone();
        bad["payload"]["command"]["auction"]["cataloguePosition"] =
            json!({"state": "SET", "value": 0});
        assert_semantically_invalid(bad);
        let mut bad = fixtures[0].clone();
        bad["payload"]["command"]["auction"]["biddingOpens"] =
            json!({"state": "SET", "value": "2026-02-04T09:00:00Z"});
        assert_semantically_invalid(bad);
        bad = fixtures[0].clone();
        bad["payload"]["command"]["images"] = json!([
            "https://example.test/image/first.jpg",
            "https://example.test/image/first.jpg"
        ]);
        assert_semantically_invalid(bad);
        let mut bad = fixtures[3].clone();
        bad["payload"]["command"]["sourceListingId"] = json!(" 陶器 🏺 / SKU-42 ");
        assert_semantically_invalid(bad);
        let mut bad = fixtures[4].clone();
        bad["payload"]["command"]["input"]["sourcePayload"] = json!(["not an object"]);
        assert_semantically_invalid(bad);
        let mut bad = fixtures[4].clone();
        bad["payload"]["command"]["providerReceipt"]["sourceEvidenceSha256"] =
            json!("not a digest");
        assert_semantically_invalid(bad);
        let mut bad = fixtures[2].clone();
        bad["payload"]["command"]["availability"] = json!({"state": "SET", "value": "sold_out"});
        assert_semantically_invalid(bad);
    }

    #[test]
    fn golden_fingerprint_tracks_patch_auction_image_order_unicode_and_raw_evidence() {
        let fixtures = golden_fixtures();
        let create = decode(&fixtures[0].to_string()).unwrap();
        let mut reversed = fixtures[0].clone();
        reversed["payload"]["command"]["images"] = json!([
            "https://example.test/image/second.jpg",
            "https://example.test/image/first.jpg"
        ]);
        let reversed = decode(&fingerprinted_value(&reversed).unwrap().to_string()).unwrap();
        assert_ne!(
            semantic_fingerprint(&create),
            semantic_fingerprint(&reversed)
        );
        assert_eq!(fifo_group_id(&create), fifo_group_id(&reversed));

        let update = decode(&fixtures[1].to_string()).unwrap();
        let mut patch = fixtures[1].clone();
        patch["payload"]["command"]["priceEstimateMin"] = json!({"state": "UNCHANGED"});
        let patch = decode(&fingerprinted_value(&patch).unwrap().to_string()).unwrap();
        assert_ne!(semantic_fingerprint(&update), semantic_fingerprint(&patch));
        let mut auction = fixtures[1].clone();
        auction["payload"]["command"]["auction"]["value"]["lotNumber"] =
            json!({"state": "SET", "value": "Lot 9"});
        let auction = decode(&fingerprinted_value(&auction).unwrap().to_string()).unwrap();
        assert_ne!(
            semantic_fingerprint(&update),
            semantic_fingerprint(&auction)
        );

        let withdraw = decode(&fixtures[3].to_string()).unwrap();
        let mut unicode_key = fixtures[3].clone();
        unicode_key["payload"]["command"]["sourceListingId"] = json!("陶器 🏺 / SKU-43");
        let unicode_key = decode(&fingerprinted_value(&unicode_key).unwrap().to_string()).unwrap();
        assert_ne!(
            semantic_fingerprint(&withdraw),
            semantic_fingerprint(&unicode_key)
        );
        assert_ne!(fifo_group_id(&withdraw), fifo_group_id(&unicode_key));

        let raw = decode(&fixtures[4].to_string()).unwrap();
        for path in ["sourcePayload", "rawValues", "normalizationContext"] {
            let mut changed = fixtures[4].clone();
            changed["payload"]["command"]["input"][path]["newFact"] = json!("青");
            let changed = decode(&fingerprinted_value(&changed).unwrap().to_string()).unwrap();
            assert_ne!(
                semantic_fingerprint(&raw),
                semantic_fingerprint(&changed),
                "{path}"
            );
            assert_eq!(fifo_group_id(&raw), fifo_group_id(&changed));
        }
        let mut changed = fixtures[4].clone();
        changed["payload"]["command"]["provenance"]["attempt"] = json!(2);
        let changed = decode(&fingerprinted_value(&changed).unwrap().to_string()).unwrap();
        assert_ne!(semantic_fingerprint(&raw), semantic_fingerprint(&changed));
    }

    #[test]
    fn all_five_operations_decode_to_typed_service_intents() {
        for (operation, builder) in [
            ("CREATE", create_command as fn(&str) -> Value),
            ("UPDATE", update_command),
            ("UPSERT", upsert_command),
            ("WITHDRAW", key),
            ("CAPTURE_RAW", raw_command),
        ] {
            let mut value = fixture(operation, Value::Null);
            let source = value["listingSourceId"].as_str().unwrap().to_owned();
            value["payload"]["command"] = builder(&source);
            let decoded = decode_value(&value).unwrap();
            let group = fifo_group_id(&decoded).unwrap();
            let dedup = fifo_deduplication_id(&decoded).unwrap();
            assert_eq!(69, group.len());
            assert_eq!(70, dedup.len());
            let intent = decoded.clone().into_intent().unwrap();
            assert_eq!(operation, intent.operation().as_str());
            assert_eq!(
                decoded
                    .listing_source_id
                    .parse::<ListingSourceId>()
                    .unwrap(),
                intent.listing_source_id()
            );
            assert_eq!(64, semantic_fingerprint(&decoded).unwrap().len());
        }
    }

    #[test]
    fn semantic_hash_and_fifo_identity_are_stable_but_distinct_commands_survive() {
        let mut value = fixture("UPDATE", Value::Null);
        let source = value["listingSourceId"].as_str().unwrap().to_owned();
        value["payload"]["command"] = update_command(&source);
        let original = decode_value(&value).unwrap();
        let mut retry = value.clone();
        retry["requestId"] = json!("another-attempt");
        retry["correlationId"] = json!("another-correlation");
        retry["preparedAt"] = json!("2026-03-01T02:03:04Z");
        let retried = decode_value(&retry).unwrap();
        assert_eq!(retried.prepared_at().unwrap().year(), 2026);
        assert_eq!(
            semantic_fingerprint(&original),
            semantic_fingerprint(&retried)
        );
        assert_eq!(
            fifo_deduplication_id(&original),
            fifo_deduplication_id(&retried)
        );
        assert_eq!(fifo_group_id(&original), fifo_group_id(&retried));
        retry["commandId"] = json!(format!("plic1_{}", "d".repeat(64)));
        assert_ne!(
            fifo_deduplication_id(&original),
            fifo_deduplication_id(&decode_value(&retry).unwrap())
        );
        retry["payload"]["command"]["priceEstimateMax"] = patch("UNCHANGED");
        let changed = decode_value(&retry).unwrap();
        assert_ne!(
            semantic_fingerprint(&original),
            semantic_fingerprint(&changed)
        );
    }

    #[test]
    fn rejects_unknown_version_fields_operation_mismatches_and_invalid_leaves() {
        let mut value = fixture("CREATE", Value::Null);
        let source = value["listingSourceId"].as_str().unwrap().to_owned();
        value["payload"]["command"] = create_command(&source);
        assert!(decode_value(&value).is_ok());
        for (path, invalid) in [
            ("schemaVersion", json!(2)),
            ("inputCount", json!(0)),
            ("commandId", json!("bad")),
            ("listingSourceId", json!(ListingSourceId::new().to_string())),
        ] {
            let mut broken = value.clone();
            broken[path] = invalid;
            assert!(decode_value(&broken).is_err());
        }
        let mut broken = value.clone();
        broken["payload"]["operation"] = json!("SHOPIFY");
        assert!(decode_value(&broken).is_err());
        broken = value.clone();
        broken["payload"]["command"]["availability"] = json!("UNKNOWN");
        assert!(decode_value(&broken).is_err());
        broken = value.clone();
        broken["payload"]["command"]["unexpected"] = json!(true);
        assert!(decode_value(&broken).is_err());
        broken = value.clone();
        broken["actor"]["id"] = json!("credential");
        assert!(decode_value(&broken).is_err());
        broken = value.clone();
        broken["payload"]["command"]["images"] =
            json!(["https://example.test/a", "https://example.test/a"]);
        assert!(decode_value(&broken).is_err());
        broken = value;
        broken["payload"]["command"]["key"]["sourceListingId"] = json!("  SKU #1 ");
        assert!(decode_value(&broken).is_err());
        let source = broken["listingSourceId"].as_str().unwrap().to_owned();
        broken["payload"]["operation"] = json!("UPDATE");
        broken["payload"]["command"] = update_command(&source);
        broken["payload"]["command"]["url"] = patch("CLEAR");
        assert!(decode_value(&broken).is_err());
        broken["payload"]["command"] = update_command(&source);
        broken["payload"]["command"]
            .as_object_mut()
            .unwrap()
            .remove("price");
        assert!(decode_value(&broken).is_err());
        broken["payload"]["command"]["price"] = json!(null);
        assert!(decode_value(&broken).is_err());
        broken["payload"]["command"]["price"] = json!({"state": "SET"});
        assert!(decode_value(&broken).is_err());
    }

    #[test]
    fn raw_evidence_is_canonical_and_grouping_is_source_scoped() {
        let mut value = fixture("CAPTURE_RAW", Value::Null);
        let source = value["listingSourceId"].as_str().unwrap().to_owned();
        value["payload"]["command"] = raw_command(&source);
        let first = decode_value(&value).unwrap();
        let first_hash = semantic_fingerprint(&first).unwrap();
        let serialized = fingerprinted_value(&value).unwrap().to_string();
        assert!(serialized.contains("\"a\":1,\"b\":2"));
        let reordered = serialized.replace("\"a\":1,\"b\":2", "\"b\":2,\"a\":1");
        let reordered_envelope = decode(&reordered).unwrap();
        assert_eq!(
            first_hash,
            semantic_fingerprint(&reordered_envelope).unwrap()
        );
        let mut changed = value.clone();
        changed["payload"]["command"]["input"]["sourcePayload"]["a"] = json!(3);
        let next = decode_value(&changed).unwrap();
        assert_ne!(first_hash, semantic_fingerprint(&next).unwrap());
        assert_eq!(fifo_group_id(&first), fifo_group_id(&next));
        changed["payload"]["command"]["sourceRecordKey"] = json!("id:2");
        assert_ne!(
            fifo_group_id(&first),
            fifo_group_id(&decode_value(&changed).unwrap())
        );
        changed = value;
        changed["payload"]["command"]["providerReceipt"]["sourceEvidenceSha256"] = json!("bad");
        assert!(decode_value(&changed).is_err());
    }

    #[test]
    fn user_and_delegated_user_share_the_v1_fingerprint_golden_vector() {
        let mut value = golden_fixtures()[3].clone();
        value["actor"] = json!({
            "kind": "USER",
            "id": "usr_01h455vb4pex5vy7enb1p677vn",
        });
        let user = decode_value(&value).unwrap();
        // Fixed SHA-256 vector for the canonical USER scope and the golden WITHDRAW payload.
        assert_eq!(
            semantic_fingerprint(&user).unwrap(),
            "001a9fbe03d64ac315f965fc0d815c0a6d9cebef60076721e0e544374cd18f6b"
        );
        value["actor"]["kind"] = json!("DELEGATED_USER");
        let delegated = decode_value(&value).unwrap();
        assert_eq!(
            semantic_fingerprint(&user),
            semantic_fingerprint(&delegated)
        );
        assert_eq!(
            fifo_deduplication_id(&user),
            fifo_deduplication_id(&delegated)
        );
        assert_ne!(user.actor().unwrap(), delegated.actor().unwrap());
    }

    #[test]
    fn fingerprint_is_actor_scoped_and_verified_before_consumer_access() {
        let mut value = fixture("WITHDRAW", Value::Null);
        let source = value["listingSourceId"].as_str().unwrap().to_owned();
        value["payload"]["command"] = key(&source);
        let system = decode_value(&value).unwrap();
        value["actor"] = json!({ "kind": "SERVICE", "id": "a-service" });
        let service = decode_value(&value).unwrap();
        assert_eq!(fifo_group_id(&system), fifo_group_id(&service));
        assert_ne!(
            semantic_fingerprint(&system),
            semantic_fingerprint(&service)
        );
        assert_ne!(
            fifo_deduplication_id(&system),
            fifo_deduplication_id(&service)
        );
        let user_id = user_core::user_id::UserId::new().to_string();
        value["actor"] = json!({ "kind": "USER", "id": user_id });
        let user = decode_value(&value).unwrap();
        assert_ne!(semantic_fingerprint(&service), semantic_fingerprint(&user));
        value["actor"]["kind"] = json!("DELEGATED_USER");
        let delegated = decode_value(&value).unwrap();
        assert_eq!(user.command_id(), delegated.command_id());
        assert_eq!(user.submission_id(), delegated.submission_id());
        assert_eq!(fifo_group_id(&user), fifo_group_id(&delegated));
        assert_ne!(user.actor().unwrap(), delegated.actor().unwrap());
        assert_eq!(
            semantic_fingerprint(&user),
            semantic_fingerprint(&delegated)
        );
        assert_eq!(
            fifo_deduplication_id(&user),
            fifo_deduplication_id(&delegated)
        );
        let mut another_user = value.clone();
        another_user["actor"]["id"] = json!(user_core::user_id::UserId::new().to_string());
        let another_user = decode_value(&another_user).unwrap();
        assert_ne!(
            semantic_fingerprint(&delegated),
            semantic_fingerprint(&another_user)
        );
        assert!(matches!(
            delegated
                .clone()
                .into_service_envelope()
                .unwrap()
                .message
                .metadata
                .actor,
            ProductListingIngestionActor::DelegatedUser(_)
        ));
        let mut retried = value.clone();
        retried["requestId"] = json!("retry-request");
        retried["correlationId"] = json!("retry-correlation");
        retried["preparedAt"] = json!("2026-01-02T00:00:00Z");
        let retried = decode_value(&retried).unwrap();
        assert_eq!(
            semantic_fingerprint(&delegated),
            semantic_fingerprint(&retried)
        );
        assert_eq!(
            fifo_deduplication_id(&delegated),
            fifo_deduplication_id(&retried)
        );

        let mut tampered = fingerprinted_value(&value).unwrap();
        tampered["payload"]["command"]["sourceListingId"] = json!("other");
        assert!(decode(&tampered.to_string()).is_err());
        tampered = fingerprinted_value(&value).unwrap();
        tampered["actor"]["id"] = json!(user_core::user_id::UserId::new().to_string());
        assert!(decode(&tampered.to_string()).is_err());
        let unverified: IngestionEnvelopeV1 = serde_json::from_value(tampered.clone()).unwrap();
        assert_eq!(unverified.verified_fingerprint(), Err(CodecError::Invalid));
        let mut same_user = fingerprinted_value(&value).unwrap();
        same_user["actor"]["kind"] = json!("USER");
        assert!(matches!(
            decode(&same_user.to_string()).unwrap().actor().unwrap(),
            ProductListingIngestionActor::User(_)
        ));
        tampered = fingerprinted_value(&value).unwrap();
        tampered["semanticFingerprint"] = json!("0".repeat(64));
        assert!(decode(&tampered.to_string()).is_err());
        tampered
            .as_object_mut()
            .unwrap()
            .remove("semanticFingerprint");
        assert!(decode(&tampered.to_string()).is_err());
    }

    #[test]
    fn prepared_timestamp_is_required_valid_and_excluded_from_fingerprint() {
        let mut value = fixture("WITHDRAW", Value::Null);
        let source = value["listingSourceId"].as_str().unwrap().to_owned();
        value["payload"]["command"] = key(&source);
        let original = decode_value(&value).unwrap();
        for invalid in [
            json!(null),
            json!("2026-01-01"),
            json!("yesterday"),
            json!("2026-01-01T01:00:00+01:00"),
        ] {
            let mut broken = fingerprinted_value(&value).unwrap();
            broken["preparedAt"] = invalid;
            assert!(decode(&broken.to_string()).is_err());
        }
        let mut missing = fingerprinted_value(&value).unwrap();
        missing.as_object_mut().unwrap().remove("preparedAt");
        assert!(decode(&missing.to_string()).is_err());
        value["preparedAt"] = json!("2026-01-02T00:00:00Z");
        let later = decode_value(&value).unwrap();
        assert_ne!(
            original.prepared_at().unwrap(),
            later.prepared_at().unwrap()
        );
        assert_eq!(
            semantic_fingerprint(&original),
            semantic_fingerprint(&later)
        );
        assert_eq!(
            fifo_deduplication_id(&original),
            fifo_deduplication_id(&later)
        );
    }

    #[test]
    fn raw_fifo_group_is_tagged_separately_from_listing_and_uses_canonical_key() {
        let mut listing = fixture("WITHDRAW", Value::Null);
        let source = listing["listingSourceId"].as_str().unwrap().to_owned();
        listing["payload"]["command"] = key(&source);
        let listing = decode_value(&listing).unwrap();
        let mut raw = fixture("CAPTURE_RAW", raw_command(&source));
        raw["listingSourceId"] = json!(source);
        raw["payload"]["command"]["sourceRecordKey"] = json!("SKU #1");
        let raw = decode_value(&raw).unwrap();
        assert_ne!(fifo_group_id(&listing), fifo_group_id(&raw));
        assert_ne!(
            fifo_group_id(&raw),
            fifo_group_id(&{
                let mut different = raw.clone();
                if let WirePayload::CaptureRaw(c) = &mut different.payload {
                    c.ingestion_method = "WEB_CRAWL".into();
                }
                different.semantic_fingerprint =
                    fingerprint(&different.actor, &different.payload).unwrap();
                different
            })
        );
        let mut noncanonical =
            fingerprinted_value(&fixture("WITHDRAW", key(&raw.listing_source_id))).unwrap();
        noncanonical["payload"]["command"]["sourceListingId"] = json!(" SKU #1 ");
        assert!(decode(&noncanonical.to_string()).is_err());
    }
}
