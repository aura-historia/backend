//! Schema descriptions for public codecs whose semantic owners do not depend on Schemars.
use schemars::{Schema, SchemaGenerator, json_schema};

pub fn object_id(prefix: &str) -> Schema {
    // UUIDv7 version and RFC variant bits in the canonical Crockford base32 suffix.
    json_schema!({"type":"string", "pattern":format!("^{prefix}_[0-7][0-9a-hjkmnp-tv-z]{{9}}[ef][0-9a-hjkmnp-tv-z]{{2}}[89abrstv][0-9a-hjkmnp-tv-z]{{12}}$")})
}
fn nullable(schema: Schema) -> Schema {
    json_schema!({"anyOf":[schema,{"type":"null"}]})
}
macro_rules! ids {
    ($($name:ident, $optional:ident, $ty:path);* $(;)?) => {$ (
        pub fn $name(_: &mut SchemaGenerator) -> Schema { object_id(<$ty>::PREFIX) }
        pub fn $optional(generator: &mut SchemaGenerator) -> Schema { nullable($name(generator)) }
    )*};
}
ids! {
    product_listing_id, optional_product_listing_id, product_listing_core::product_listing_id::ProductListingId;
    event_id, optional_event_id, domain_primitives::event_id::EventId;
    listing_source_id, optional_listing_source_id, listing_source_core::ListingSourceId;
    auction_id, optional_auction_id, auction_core::AuctionId;
    fx_rate_id, optional_fx_rate_id, fxrate_core::FxRateId;
    notification_id, optional_notification_id, notification_core::notification_id::NotificationId;
    user_search_filter_id, optional_user_search_filter_id, search_filter_core::user_search_filter_id::UserSearchFilterId;
    party_location_id, optional_party_location_id, party_core::party_location_id::PartyLocationId;
}
pub fn notification_ids(generator: &mut SchemaGenerator) -> Schema {
    json_schema!({"type":"array", "items":notification_id(generator)})
}
pub fn optional_distance(_: &mut SchemaGenerator) -> Schema {
    nullable(
        json_schema!({"type":"object", "additionalProperties":false,"required":["amount","unit"],"properties":{
            "amount":{"type":"number"},"unit":{"type":"string"}
        }}),
    )
}
pub fn slug(_: &mut SchemaGenerator) -> Schema {
    json_schema!({"type":"string","maxLength":120,"pattern":"^[a-z0-9]+(?:-[a-z0-9]+)*-[0-9a-f]{6}$"})
}

pub fn datetime(_: &mut SchemaGenerator) -> Schema {
    json_schema!({"type":"string","format":"date-time"})
}
pub fn optional_datetime(generator: &mut SchemaGenerator) -> Schema {
    nullable(datetime(generator))
}
pub fn url(_: &mut SchemaGenerator) -> Schema {
    json_schema!({"type":"string","format":"uri"})
}
pub fn optional_url(generator: &mut SchemaGenerator) -> Schema {
    nullable(url(generator))
}
pub fn product_listing_ids(generator: &mut SchemaGenerator) -> Schema {
    json_schema!({"type":"array","items":product_listing_id(generator)})
}
pub fn listing_source_ids(generator: &mut SchemaGenerator) -> Schema {
    json_schema!({"type":"array","items":listing_source_id(generator)})
}
pub fn auction_ids(generator: &mut SchemaGenerator) -> Schema {
    json_schema!({"type":"array","items":auction_id(generator)})
}
