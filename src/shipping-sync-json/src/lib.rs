//! Pinned SHIP_SYNC_V1 evidence protocol. Never accepts arbitrary provider settings.
use application::error::{BoxError, box_error};
use listing_source_core::ListingSourceId;
use party_core::{party_id::PartyId, party_location_id::PartyLocationId};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use shipping_core::{
    profile::{ShippingProfileContent, ShippingProfileId, ShippingProfileName},
    selection::DeliveryOverrides,
};
use shipping_json::{
    ShippingTermsReferenceCodec,
    listing_delivery::{self, DeliveryReferenceCodec},
};
use shipping_service::ports::ShippingEvidence;
use shipping_sync_service::*;
use time::{OffsetDateTime, format_description::well_known::Rfc3339};

pub struct PublicReferences;
impl ShippingTermsReferenceCodec for PublicReferences {
    fn decode_party(v: &str) -> Result<PartyId, BoxError> {
        v.parse().map_err(box_error)
    }
    fn encode_party(v: PartyId) -> String {
        v.to_string()
    }
    fn decode_location(v: &str) -> Result<PartyLocationId, BoxError> {
        v.parse().map_err(box_error)
    }
    fn encode_location(v: PartyLocationId) -> String {
        v.to_string()
    }
}
impl DeliveryReferenceCodec for PublicReferences {
    fn decode_source(v: &str) -> Result<ListingSourceId, BoxError> {
        v.parse().map_err(box_error)
    }
    fn encode_source(v: ListingSourceId) -> String {
        v.to_string()
    }
    fn decode_profile(v: &str) -> Result<ShippingProfileId, BoxError> {
        v.parse().map_err(box_error)
    }
    fn encode_profile(v: ShippingProfileId) -> String {
        v.to_string()
    }
}
fn bad() -> BoxError {
    box_error(std::io::Error::other("invalid shipping sync protocol"))
}
fn code(v: &str) -> Result<ObjectKind, BoxError> {
    match v {
        "PROFILE" => Ok(ObjectKind::Profile),
        "DEFAULT" => Ok(ObjectKind::Default),
        "ITEM" => Ok(ObjectKind::Item),
        _ => Err(bad()),
    }
}
pub fn hex(digest: &[u8; 32]) -> String {
    digest.iter().map(|b| format!("{b:02x}")).collect()
}
pub fn digest(v: &str) -> Result<[u8; 32], BoxError> {
    if v.len() != 64
        || !v
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err(bad());
    }
    let mut out = [0; 32];
    for (i, byte) in out.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&v[i * 2..i * 2 + 2], 16)?;
    }
    Ok(out)
}
pub fn timestamp(value: &str) -> Result<OffsetDateTime, BoxError> {
    OffsetDateTime::parse(value, &Rfc3339).map_err(box_error)
}
fn instant(value: OffsetDateTime) -> Result<String, BoxError> {
    value.format(&Rfc3339).map_err(box_error)
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct WindowData {
    observed_at: String,
    fresh_until: String,
    expires_at: String,
}
impl WindowData {
    fn decode(self) -> Result<ObservationWindow, BoxError> {
        let window = ObservationWindow {
            observed_at: timestamp(&self.observed_at)?,
            fresh_until: timestamp(&self.fresh_until)?,
            expires_at: timestamp(&self.expires_at)?,
        };
        if window.fresh_until < window.observed_at || window.expires_at < window.fresh_until {
            return Err(bad());
        }
        Ok(window)
    }
    fn encode(v: ObservationWindow) -> Result<Self, BoxError> {
        Ok(Self {
            observed_at: instant(v.observed_at)?,
            fresh_until: instant(v.fresh_until)?,
            expires_at: instant(v.expires_at)?,
        })
    }
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct EvidenceData {
    reference: String,
    assertion: String,
}
#[derive(Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "SCREAMING_SNAKE_CASE", deny_unknown_fields)]
enum SelectionData {
    Unresolved,
    Unavailable,
    Profile { id: String },
    InheritSourceDefault,
    SuppressInheritance,
}
#[derive(Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "SCREAMING_SNAKE_CASE", deny_unknown_fields)]
enum AssertionData {
    Profile {
        name: String,
        terms: Value,
        evidence: Vec<EvidenceData>,
    },
    Default {
        #[serde(deserialize_with = "required")]
        profile: Nullable<String>,
    },
    Item {
        source_listing_id: String,
        selection: SelectionData,
        rate_inputs: Value,
        shipping: Value,
        collection: Value,
    },
    Delete {
        object_kind: String,
    },
    Unavailable {
        capability: String,
        reason: String,
    },
    ShopifyProfile {
        profile_id: String,
        name: String,
        zones: Vec<ProviderZone>,
    },
    ShopifyMembership {
        product_id: String,
        complete: bool,
        variants: Vec<VariantProfile>,
    },
    WoocommerceConfiguration {
        configuration_id: String,
        name: String,
        zones: Vec<ProviderZone>,
    },
    WoocommerceItem {
        product_id: String,
        class_ids: Vec<String>,
        classes_complete: bool,
        variation_ids: Vec<String>,
        profile: String,
    },
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ProviderZone {
    id: String,
    countries: Vec<String>,
    methods: Vec<ProviderMethod>,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ProviderMethod {
    id: String,
    #[serde(deserialize_with = "required")]
    class_id: Nullable<String>,
    amount_minor: u64,
    currency: String,
    basis: String,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct VariantProfile {
    id: String,
    profile: String,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ObjectData {
    kind: String,
    id: String,
    #[serde(deserialize_with = "required")]
    parent: Nullable<String>,
    #[serde(deserialize_with = "required")]
    profile: Nullable<String>,
}
// Required deserializers distinguish omitted assertions from explicit null.
#[derive(Serialize, Deserialize)]
#[serde(transparent)]
struct Nullable<T>(Option<T>);
fn required<'de, D: serde::Deserializer<'de>, T: Deserialize<'de>>(
    d: D,
) -> Result<Nullable<T>, D::Error> {
    Option::deserialize(d).map(Nullable)
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RecordData {
    key: String,
    window: WindowData,
    objects: Vec<ObjectData>,
    assertion: AssertionData,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ChunkData {
    schema: String,
    records: Vec<RecordData>,
}

fn shopify_product_key(
    product_id: &str,
) -> Result<product_listing_core::source_listing_id::SourceListingId, BoxError> {
    let numeric = product_id
        .strip_prefix("gid://shopify/Product/")
        .unwrap_or(product_id);
    let id: u64 = numeric.parse()?;
    if id == 0 || id.to_string() != numeric {
        return Err(bad());
    }
    // Existing Shopify webhook ingestion and connector backfill use numeric source
    // listing IDs. Retain the native provider identity separately as provenance.
    numeric.to_owned().try_into().map_err(box_error)
}

fn decode_record<C: DeliveryReferenceCodec>(
    r: RecordData,
    source: ListingSourceId,
) -> Result<Record, BoxError> {
    if r.objects.len() > 64 {
        return Err(bad());
    }
    let mut objects = r
        .objects
        .into_iter()
        .map(|o| {
            Ok(ObservedObject {
                kind: match o.kind.as_str() {
                    "PROFILE" => ObservedObjectKind::Profile,
                    "ZONE" => ObservedObjectKind::Zone,
                    "METHOD" => ObservedObjectKind::Method,
                    "CLASS" => ObservedObjectKind::Class,
                    "PRODUCT" => ObservedObjectKind::Product,
                    "VARIANT" => ObservedObjectKind::Variant,
                    _ => return Err(bad()),
                },
                id: ExternalKey::new(o.id)?,
                parent: o.parent.0.map(ExternalKey::new).transpose()?,
                profile: o.profile.0.map(ExternalKey::new).transpose()?,
            })
        })
        .collect::<Result<Vec<_>, BoxError>>()?;
    let key = ExternalKey::new(r.key)?;
    let window = r.window.decode()?;
    let assertion = match r.assertion {
        AssertionData::Profile {
            name,
            terms,
            evidence,
        } => {
            if evidence.len() > 32 {
                return Err(bad());
            }
            let evidence = evidence
                .into_iter()
                .map(|e| ShippingEvidence::new(e.reference, e.assertion))
                .collect::<Result<Vec<_>, _>>()?;
            if evidence
                .iter()
                .enumerate()
                .any(|(i, e)| evidence[..i].contains(e))
            {
                return Err(bad());
            }
            Assertion::Profile {
                content: Box::new(ShippingProfileContent {
                    name: ShippingProfileName::new(name)?,
                    attributed_party: None,
                    terms: shipping_json::decode::<C>(terms, source)?,
                }),
                evidence,
            }
        }
        AssertionData::Default { profile } => Assertion::Default {
            profile: profile.0.map(ExternalKey::new).transpose()?,
        },
        AssertionData::Item {
            source_listing_id,
            selection,
            rate_inputs,
            shipping,
            collection,
        } => {
            let source_listing_id = source_listing_id.try_into()?;
            let assertion = ItemAssertion {
                selection: match selection {
                    SelectionData::Unresolved => EvidenceSelection::Unresolved,
                    SelectionData::Unavailable => EvidenceSelection::Unavailable,
                    SelectionData::Profile { id } => {
                        EvidenceSelection::Profile(ExternalKey::new(id)?)
                    }
                    SelectionData::InheritSourceDefault => EvidenceSelection::InheritSourceDefault,
                    SelectionData::SuppressInheritance => EvidenceSelection::SuppressInheritance,
                },
                rate_inputs: listing_delivery::decode_inputs(rate_inputs, source)?,
                overrides: DeliveryOverrides {
                    shipping: listing_delivery::decode_shipping::<C>(shipping, source)?,
                    collection: listing_delivery::decode_collection::<C>(collection, source)?,
                },
            };
            // Validate all source-scoped leaves even if membership is unavailable.
            product_listing_core::delivery::ListingDelivery::new(
                source,
                Default::default(),
                None,
                assertion.rate_inputs.clone(),
                assertion.overrides.clone(),
            )?;
            Assertion::Item {
                source_listing_id,
                assertion: Box::new(assertion),
            }
        }
        AssertionData::Delete { object_kind } => Assertion::Delete {
            kind: code(&object_kind)?,
        },
        AssertionData::Unavailable { capability, reason } => Assertion::Unavailable {
            capability: match capability.as_str() {
                "CONFIGURATION" => Capability::Configuration,
                "MEMBERSHIP" => Capability::Membership,
                "RATE_INPUTS" => Capability::RateInputs,
                _ => return Err(bad()),
            },
            reason: match reason.as_str() {
                "PERMISSION_DENIED" => UnavailableReason::PermissionDenied,
                "UNSUPPORTED" => UnavailableReason::Unsupported,
                "EXTRACTION_FAILED" => UnavailableReason::ExtractionFailed,
                _ => return Err(bad()),
            },
        },
        AssertionData::ShopifyProfile {
            profile_id,
            name,
            zones,
        } => {
            if key.as_str() != profile_id || !objects.is_empty() {
                return Err(bad());
            }
            let terms = provider_terms::<C>(zones, source, false, &mut objects)?;
            objects.push(ObservedObject {
                kind: ObservedObjectKind::Profile,
                id: key.clone(),
                parent: None,
                profile: None,
            });
            Assertion::Profile {
                content: Box::new(ShippingProfileContent {
                    name: ShippingProfileName::new(name)?,
                    attributed_party: None,
                    terms,
                }),
                evidence: vec![],
            }
        }
        AssertionData::WoocommerceConfiguration {
            configuration_id,
            name,
            zones,
        } => {
            if key.as_str() != configuration_id || !objects.is_empty() {
                return Err(bad());
            }
            let terms = provider_terms::<C>(zones, source, true, &mut objects)?;
            Assertion::Profile {
                content: Box::new(ShippingProfileContent {
                    name: ShippingProfileName::new(name)?,
                    attributed_party: None,
                    terms,
                }),
                evidence: vec![],
            }
        }
        AssertionData::ShopifyMembership {
            product_id,
            complete,
            variants,
        } => {
            let source_listing_id = shopify_product_key(&product_id)?;
            if key.as_str() != source_listing_id.as_ref()
                || variants.is_empty()
                || variants.len() > 32
                || !objects.is_empty()
            {
                return Err(bad());
            }
            let common = variants
                .first()
                .filter(|first| complete && variants.iter().all(|v| v.profile == first.profile))
                .map(|v| ExternalKey::new(v.profile.clone()))
                .transpose()?;
            let ids = variants.iter().map(|v| v.id.clone()).collect::<Vec<_>>();
            let product = ExternalKey::new(product_id)?;
            objects.push(ObservedObject {
                kind: ObservedObjectKind::Product,
                id: product.clone(),
                parent: None,
                profile: None,
            });
            for v in variants {
                objects.push(ObservedObject {
                    kind: ObservedObjectKind::Variant,
                    id: ExternalKey::new(v.id)?,
                    parent: Some(product.clone()),
                    profile: Some(ExternalKey::new(v.profile)?),
                });
            }
            Assertion::Item {
                source_listing_id,
                assertion: Box::new(ItemAssertion {
                    selection: if complete {
                        common
                            .map(EvidenceSelection::Profile)
                            .unwrap_or(EvidenceSelection::Unresolved)
                    } else {
                        EvidenceSelection::Unavailable
                    },
                    rate_inputs: listing_delivery::decode_inputs(
                        json!({"classes":null,"variants":if complete{Some(ids)}else{None}}),
                        source,
                    )?,
                    overrides: Default::default(),
                }),
            }
        }
        AssertionData::WoocommerceItem {
            product_id,
            class_ids,
            classes_complete,
            variation_ids,
            profile,
        } => {
            if key.as_str() != product_id
                || class_ids.len() > 32
                || variation_ids.len() > 32
                || !objects.is_empty()
            {
                return Err(bad());
            }
            for id in &class_ids {
                objects.push(ObservedObject {
                    kind: ObservedObjectKind::Class,
                    id: ExternalKey::new(id.clone())?,
                    parent: None,
                    profile: None,
                });
            }
            for id in &variation_ids {
                objects.push(ObservedObject {
                    kind: ObservedObjectKind::Variant,
                    id: ExternalKey::new(id.clone())?,
                    parent: Some(key.clone()),
                    profile: None,
                });
            }
            Assertion::Item {
                source_listing_id: product_id.try_into()?,
                assertion: Box::new(ItemAssertion {
                    selection: if classes_complete {
                        EvidenceSelection::Profile(ExternalKey::new(profile)?)
                    } else {
                        EvidenceSelection::Unavailable
                    },
                    rate_inputs: listing_delivery::decode_inputs(
                        json!({"classes":{"codes":class_ids,"completeness":if classes_complete{"COMPLETE"}else{"PARTIAL"}},"variants":if variation_ids.is_empty(){None}else{Some(variation_ids)}}),
                        source,
                    )?,
                    overrides: Default::default(),
                }),
            }
        }
    };
    if objects.len() > 64 {
        return Err(bad());
    }
    Ok(Record {
        key,
        window,
        assertion,
        objects,
    })
}
fn provider_terms<C: DeliveryReferenceCodec>(
    zones: Vec<ProviderZone>,
    source: ListingSourceId,
    classes: bool,
    objects: &mut Vec<ObservedObject>,
) -> Result<shipping_core::terms::DeliveryTerms, BoxError> {
    if zones.len() > 16 {
        return Err(bad());
    }
    let applicability = json!({"class":null,"variants":null,"quantity":null,"threshold":null});
    let mut mapped = Vec::new();
    for z in zones {
        if z.countries.is_empty()
            || z.countries.len() > 32
            || z.methods.is_empty()
            || z.methods.len() > 16
        {
            return Err(bad());
        }
        let zone = ExternalKey::new(z.id)?;
        objects.push(ObservedObject {
            kind: ObservedObjectKind::Zone,
            id: zone.clone(),
            parent: None,
            profile: None,
        });
        let mut services = Vec::new();
        for m in z.methods {
            if !classes && m.class_id.0.is_some() {
                return Err(bad());
            }
            let id = ExternalKey::new(m.id)?;
            objects.push(ObservedObject {
                kind: ObservedObjectKind::Method,
                id: id.clone(),
                parent: Some(zone.clone()),
                profile: None,
            });
            let mut applies = applicability.clone();
            if let Some(class) = m.class_id.0 {
                ExternalKey::new(class.clone())?;
                applies["class"] = json!(class);
            }
            // Native candidates supply known advertised facts. No expression evaluation,
            // currency guessing, first-variant shortcut or checkout total is performed.
            services.push(json!({"code":id.as_str(),"applicability":applies,"eligibility":"OFFERED","rate":{"price":{"kind":"FIXED","amount":{"amount_minor":m.amount_minor,"currency":m.currency}},"basis":m.basis,"qualifying_scope":null,"components":{}},"lead_times":{"handling":null,"transit":null,"total":null},"features":[],"arranger":null,"performer":null}));
        }
        mapped.push(json!({"areas":z.countries.into_iter().map(|c|json!({"kind":"COUNTRY","code":c,"reference_release":"iso-codes-4.20.1"})).collect::<Vec<_>>(),"applicability":applicability,"services":services}));
    }
    shipping_json::decode::<C>(
        json!({"schema":"SHIP_TERMS_V1","shipping":{"kind":"COVERAGE","coverage":{"reference_release":"iso-codes-4.20.1","completeness":"PARTIAL","applicability":applicability,"exclusions":[],"dispatch_origin":null,"zones":mapped}},"collection":null}),
        source,
    )
}
fn encode_record<C: DeliveryReferenceCodec>(r: &Record) -> Result<RecordData, BoxError> {
    Ok(RecordData {
        key: r.key.as_str().into(),
        window: WindowData::encode(r.window)?,
        objects: r
            .objects
            .iter()
            .map(|o| ObjectData {
                kind: match o.kind {
                    ObservedObjectKind::Profile => "PROFILE",
                    ObservedObjectKind::Zone => "ZONE",
                    ObservedObjectKind::Method => "METHOD",
                    ObservedObjectKind::Class => "CLASS",
                    ObservedObjectKind::Product => "PRODUCT",
                    ObservedObjectKind::Variant => "VARIANT",
                }
                .into(),
                id: o.id.as_str().into(),
                parent: Nullable(o.parent.as_ref().map(|k| k.as_str().into())),
                profile: Nullable(o.profile.as_ref().map(|k| k.as_str().into())),
            })
            .collect(),
        assertion: match &r.assertion {
            Assertion::Profile { content, evidence } => {
                if content.attributed_party.is_some() {
                    return Err(bad());
                }
                AssertionData::Profile {
                    name: content.name.as_str().into(),
                    terms: shipping_json::encode::<C>(&content.terms)?,
                    evidence: evidence
                        .iter()
                        .map(|e| EvidenceData {
                            reference: e.reference().into(),
                            assertion: e.assertion().into(),
                        })
                        .collect(),
                }
            }
            Assertion::Default { profile } => AssertionData::Default {
                profile: Nullable(profile.as_ref().map(|k| k.as_str().into())),
            },
            Assertion::Item {
                source_listing_id,
                assertion,
            } => AssertionData::Item {
                source_listing_id: source_listing_id.to_string(),
                selection: match &assertion.selection {
                    EvidenceSelection::Unresolved => SelectionData::Unresolved,
                    EvidenceSelection::Unavailable => SelectionData::Unavailable,
                    EvidenceSelection::Profile(k) => SelectionData::Profile {
                        id: k.as_str().into(),
                    },
                    EvidenceSelection::InheritSourceDefault => SelectionData::InheritSourceDefault,
                    EvidenceSelection::SuppressInheritance => SelectionData::SuppressInheritance,
                },
                rate_inputs: listing_delivery::encode_inputs(&assertion.rate_inputs)?,
                shipping: listing_delivery::encode_shipping::<C>(&assertion.overrides.shipping)?,
                collection: listing_delivery::encode_collection::<C>(
                    &assertion.overrides.collection,
                )?,
            },
            Assertion::Delete { kind } => AssertionData::Delete {
                object_kind: kind.as_str().into(),
            },
            Assertion::Unavailable { capability, reason } => AssertionData::Unavailable {
                capability: match capability {
                    Capability::Configuration => "CONFIGURATION",
                    Capability::Membership => "MEMBERSHIP",
                    Capability::RateInputs => "RATE_INPUTS",
                }
                .into(),
                reason: match reason {
                    UnavailableReason::PermissionDenied => "PERMISSION_DENIED",
                    UnavailableReason::Unsupported => "UNSUPPORTED",
                    UnavailableReason::ExtractionFailed => "EXTRACTION_FAILED",
                }
                .into(),
            },
        },
    })
}
pub fn encode_chunk<C: DeliveryReferenceCodec>(records: &[Record]) -> Result<Value, BoxError> {
    serde_json::to_value(ChunkData {
        schema: "SHIP_SYNC_V1".into(),
        records: records
            .iter()
            .map(encode_record::<C>)
            .collect::<Result<_, _>>()?,
    })
    .map_err(box_error)
}
pub fn decode_chunk<C: DeliveryReferenceCodec>(
    value: Value,
    source: ListingSourceId,
) -> Result<Chunk, BoxError> {
    let data: ChunkData = serde_json::from_value(value)?;
    if data.schema != "SHIP_SYNC_V1" || data.records.len() > MAX_CHUNK_RECORDS {
        return Err(bad());
    }
    let records = data
        .records
        .into_iter()
        .map(|r| decode_record::<C>(r, source))
        .collect::<Result<Vec<_>, _>>()?;
    let bytes = canonical_bytes(&encode_chunk::<PublicReferences>(&records)?)?;
    if bytes.len() > MAX_CHUNK_BYTES {
        return Err(bad());
    }
    Ok(Chunk {
        digest: Sha256::digest(bytes).into(),
        records,
    })
}
pub fn parse_chunk(body: &[u8], source: ListingSourceId) -> Result<Chunk, BoxError> {
    if body.len() > MAX_CHUNK_BYTES {
        return Err(bad());
    }
    decode_chunk::<PublicReferences>(serde_json::from_slice(body)?, source)
}
pub fn canonical_bytes(v: &Value) -> Result<Vec<u8>, BoxError> {
    // Explicit sorting remains deterministic with serde_json's preserve_order feature enabled.
    fn write(v: &Value, out: &mut Vec<u8>) -> Result<(), BoxError> {
        match v {
            Value::Object(m) => {
                out.push(b'{');
                let mut keys = m.keys().collect::<Vec<_>>();
                keys.sort();
                for (i, k) in keys.iter().enumerate() {
                    if i > 0 {
                        out.push(b',');
                    }
                    serde_json::to_writer(&mut *out, k)?;
                    out.push(b':');
                    write(&m[*k], out)?;
                }
                out.push(b'}');
            }
            Value::Array(a) => {
                out.push(b'[');
                for (i, v) in a.iter().enumerate() {
                    if i > 0 {
                        out.push(b',');
                    }
                    write(v, out)?;
                }
                out.push(b']');
            }
            _ => serde_json::to_writer(out, v)?,
        }
        Ok(())
    }
    let mut out = Vec::new();
    write(v, &mut out)?;
    Ok(out)
}
pub fn manifest(chunks: &[Chunk]) -> Manifest {
    Manifest {
        chunks: chunks.len() as u32,
        records: chunks.iter().map(|c| c.records.len() as u32).sum(),
        digest: Sha256::digest(chunks.iter().flat_map(|c| c.digest).collect::<Vec<_>>()).into(),
    }
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "SCREAMING_SNAKE_CASE", deny_unknown_fields)]
enum AppliedData {
    Profile {
        id: String,
        revision: u64,
    },
    Default {
        revision: u64,
        #[serde(deserialize_with = "required")]
        profile: Nullable<String>,
    },
    Item {
        source_listing_id: String,
        delivery: Value,
        owned: [bool; 4],
    },
    Tombstone,
}
pub fn encode_applied<C: DeliveryReferenceCodec>(value: &AppliedValue) -> Result<Value, BoxError> {
    Ok(
        json!({"schema":"SHIP_SYNC_APPLIED_V1", "value": match value {
            AppliedValue::Profile { id, revision } => serde_json::to_value(AppliedData::Profile { id: C::encode_profile(*id), revision: *revision })?,
            AppliedValue::Default { revision, profile } => serde_json::to_value(AppliedData::Default { revision: *revision, profile: Nullable(profile.map(C::encode_profile)) })?,
            AppliedValue::Item { source_listing_id, delivery, owned } => serde_json::to_value(AppliedData::Item { source_listing_id: source_listing_id.to_string(), delivery: listing_delivery::encode::<C>(delivery)?, owned: *owned })?,
            AppliedValue::Tombstone => serde_json::to_value(AppliedData::Tombstone)?,
        }}),
    )
}
pub fn decode_applied<C: DeliveryReferenceCodec>(value: Value) -> Result<AppliedValue, BoxError> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Envelope {
        schema: String,
        value: AppliedData,
    }
    let e: Envelope = serde_json::from_value(value)?;
    if e.schema != "SHIP_SYNC_APPLIED_V1" {
        return Err(bad());
    }
    Ok(match e.value {
        AppliedData::Profile { id, revision } => {
            if revision == 0 || revision > i64::MAX as u64 {
                return Err(bad());
            }
            AppliedValue::Profile {
                id: C::decode_profile(&id)?,
                revision,
            }
        }
        AppliedData::Default { revision, profile } => {
            if revision == 0 || revision > i64::MAX as u64 {
                return Err(bad());
            }
            AppliedValue::Default {
                revision,
                profile: profile.0.map(|p| C::decode_profile(&p)).transpose()?,
            }
        }
        AppliedData::Item {
            source_listing_id,
            delivery,
            owned,
        } => AppliedValue::Item {
            source_listing_id: source_listing_id.try_into()?,
            delivery: Box::new(listing_delivery::decode::<C>(delivery)?),
            owned,
        },
        AppliedData::Tombstone => AppliedValue::Tombstone,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    const FIXTURES: &[&str] = &[
        include_str!("../../../docs/contracts/shipping-sync-v1/fixtures/profile-8000.json"),
        include_str!("../../../docs/contracts/shipping-sync-v1/fixtures/profile-9500.json"),
        include_str!("../../../docs/contracts/shipping-sync-v1/fixtures/default.json"),
        include_str!("../../../docs/contracts/shipping-sync-v1/fixtures/membership.json"),
        include_str!(
            "../../../docs/contracts/shipping-sync-v1/fixtures/shipping-only-override.json"
        ),
        include_str!("../../../docs/contracts/shipping-sync-v1/fixtures/complete-snapshot.json"),
        include_str!("../../../docs/contracts/shipping-sync-v1/fixtures/delta.json"),
        include_str!("../../../docs/contracts/shipping-sync-v1/fixtures/deletion.json"),
        include_str!("../../../docs/contracts/shipping-sync-v1/fixtures/unavailable.json"),
        include_str!(
            "../../../docs/contracts/shipping-sync-v1/fixtures/shopify-variant-membership.json"
        ),
        include_str!("../../../docs/contracts/shipping-sync-v1/fixtures/shopify-profile.json"),
        include_str!(
            "../../../docs/contracts/shipping-sync-v1/fixtures/woocommerce-class-rates.json"
        ),
        include_str!(
            "../../../docs/contracts/shipping-sync-v1/fixtures/shopify-incomplete-membership.json"
        ),
    ];
    fn source() -> ListingSourceId {
        "ls_01h455vb4pex5vy7enb1p677v1".parse().unwrap()
    }
    #[test]
    fn pinned_fixtures_are_executable_and_canonical_hashes_survive_reencoding() {
        for fixture in FIXTURES {
            let chunk = parse_chunk(fixture.as_bytes(), source()).unwrap();
            let persisted = encode_chunk::<PublicReferences>(&chunk.records).unwrap();
            assert_eq!(
                chunk,
                decode_chunk::<PublicReferences>(persisted, source()).unwrap()
            );
        }
        let invalid =
            include_str!("../../../docs/contracts/shipping-sync-v1/fixtures/malformed.json");
        assert!(parse_chunk(invalid.as_bytes(), source()).is_err());
    }
    #[test]
    fn gallery_minor_units_and_provider_membership_remain_conditional() {
        for (text, amount) in [(FIXTURES[0], 8000), (FIXTURES[1], 9500)] {
            let chunk = parse_chunk(text.as_bytes(), source()).unwrap();
            let Assertion::Profile { content, .. } = &chunk.records[0].assertion else {
                panic!("profile")
            };
            assert_eq!(
                shipping_core::fixtures::gallery_terms(amount),
                content.terms
            );
        }
        let chunk = parse_chunk(FIXTURES[9].as_bytes(), source()).unwrap();
        let Assertion::Item { assertion, .. } = &chunk.records[0].assertion else {
            panic!("item")
        };
        assert_eq!(EvidenceSelection::Unresolved, assertion.selection);
        let partial = parse_chunk(FIXTURES[12].as_bytes(), source()).unwrap();
        let Assertion::Item { assertion, .. } = &partial.records[0].assertion else {
            panic!("item")
        };
        assert_eq!(EvidenceSelection::Unavailable, assertion.selection);
        assert_eq!(3, chunk.records[0].objects.len());
        assert!(
            chunk.records[0].objects[1..]
                .iter()
                .all(|o| o.profile.is_some())
        );
        let chunk = parse_chunk(FIXTURES[11].as_bytes(), source()).unwrap();
        let Assertion::Profile { content, .. } = &chunk.records[0].assertion else {
            panic!("profile")
        };
        let shipping_core::terms::ShippingPolicy::Coverage(coverage) =
            content.terms.shipping().unwrap()
        else {
            panic!("coverage")
        };
        assert_eq!(
            Some("painting"),
            coverage.zones()[0].services()[0]
                .applicability()
                .class()
                .map(|c| c.code())
        );
        assert_eq!(
            Some("oversized"),
            coverage.zones()[0].services()[1]
                .applicability()
                .class()
                .map(|c| c.code())
        );
    }
    #[test]
    fn strict_nested_shapes_unknown_versions_and_oversized_input_fail_closed() {
        let mut absent: Value = serde_json::from_str(FIXTURES[2]).unwrap();
        absent["records"][0]["assertion"]
            .as_object_mut()
            .unwrap()
            .remove("profile");
        assert!(decode_chunk::<PublicReferences>(absent, source()).is_err());
        let mut value: Value = serde_json::from_str(FIXTURES[10]).unwrap();
        value["records"][0]["assertion"]["zones"][0]["methods"][0]["expression"] =
            json!("[qty]*[fee]");
        assert!(decode_chunk::<PublicReferences>(value, source()).is_err());
        let mut value: Value = serde_json::from_str(FIXTURES[0]).unwrap();
        value["schema"] = json!("SHIP_SYNC_V2");
        assert!(decode_chunk::<PublicReferences>(value, source()).is_err());
        assert!(parse_chunk(&vec![b' '; MAX_CHUNK_BYTES + 1], source()).is_err());
        assert!(digest(&"A".repeat(64)).is_err());
        assert!(digest(&"z".repeat(64)).is_err());
    }

    #[test]
    fn shopify_membership_resolves_existing_catalog_keys_and_preserves_native_identity() {
        let mut value: Value = serde_json::from_str(FIXTURES[9]).unwrap();
        for product_id in ["gid://shopify/Product/100", "100"] {
            value["records"][0]["assertion"]["product_id"] = json!(product_id);
            let chunk = decode_chunk::<PublicReferences>(value.clone(), source()).unwrap();
            let record = &chunk.records[0];
            let Assertion::Item {
                source_listing_id, ..
            } = &record.assertion
            else {
                panic!("item")
            };
            assert_eq!("100", source_listing_id.as_ref());
            assert_eq!(product_id, record.objects[0].id.as_str());
            assert!(
                record.objects[1..]
                    .iter()
                    .all(|o| o.parent.as_ref() == Some(&record.objects[0].id))
            );
        }
        for invalid in [
            "gid://shopify/ProductVariant/100",
            "gid://shopify/Product/0100",
            "0",
            "+100",
            "18446744073709551616",
        ] {
            value["records"][0]["assertion"]["product_id"] = json!(invalid);
            assert!(decode_chunk::<PublicReferences>(value.clone(), source()).is_err());
        }
        value["records"][0]["assertion"]["product_id"] = json!("gid://shopify/Product/101");
        assert!(decode_chunk::<PublicReferences>(value, source()).is_err());
    }
}
