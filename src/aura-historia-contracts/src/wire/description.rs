//! Parameter descriptions that several public contracts share, so a field reads the same
//! wherever it appears (the listing search and both saved-search shapes, for example).
//!
//! Each text states what the services do with the value. The generated schema already lists the
//! accepted codes, so the texts do not repeat them.

macro_rules! minor_unit_rule {
    () => {
        "Amounts are always integers in the currency's minor unit when one exists (EUR `119199` = €1,191.99; USD in cents). For currencies without minor units, such as JPY, the amount is whole units (`211453` = ¥211,453). Follow ISO 4217 exponents."
    };
}

/// How every money amount in a request or response is encoded. The MCP registry appends it to the
/// description of each tool whose schemas contain a money amount.
pub const MINOR_UNIT_RULE: &str = minor_unit_rule!();

/// The link people should follow to a provider's page. Shared by every result that pairs `url`
/// with `viewUrl`, so agents read the same rule wherever a link appears.
pub const VIEW_URL: &str = "Link to the provider's page that is meant to be shared with people. It carries Aura Historia's referral tagging, which is lost when its query parameters are changed.";
/// The provider's own address next to `viewUrl`.
pub const SOURCE_URL: &str = "The provider's own address, without Aura Historia's referral tagging; `viewUrl` is the link meant to be shared.";

/// Which provider link a result means to be shared, stated as a fact about the result. The MCP
/// registry appends it to the description of each tool whose result contains a `viewUrl`.
pub const LINK_RULE: &str = "Each result's `viewUrl` is the provider link meant to be shared with people: it carries Aura Historia's referral tagging, which is lost when its query parameters are changed. `url` is the provider's untagged address.";

pub const PRODUCT_QUERY: &str = "Keyword phrases matched against listing titles; every word of one phrase must match, and a listing matches when any phrase does.";
pub const ENHANCED_SEARCH_DESCRIPTION: &str = "Optional natural-language description that refines the ranking of `productQuery` results. Only effective together with `productQuery`; on its own it returns unranked, unrelated results.";
pub const EXCLUDE_PRODUCT_LISTING_ID: &str = "Listing IDs (`pl_…`) to leave out of the results.";
pub const LISTING_SOURCE_ID: &str =
    "Only listings from these providers, given as listing source IDs (`ls_…`).";
pub const EXCLUDE_LISTING_SOURCE_ID: &str =
    "Leave out listings from these providers, given as listing source IDs (`ls_…`).";
pub const AUCTION_ID: &str =
    "Only listings that belong to these auctions, given as auction IDs (`auc_…`).";
pub const PRICE_RANGE: &str = concat!(
    "Price range in the request's `currency`, with both bounds inclusive. ",
    minor_unit_rule!()
);
pub const PRICE_MIN: &str =
    "Lowest price to include, as an integer amount in the currency's minor unit.";
pub const PRICE_MAX: &str =
    "Highest price to include, as an integer amount in the currency's minor unit.";
pub const AVAILABILITY: &str = "Only listings in one of these exact availability states; combined with `orderability`, a listing must satisfy both.";
pub const ORDERABILITY: &str = "Only listings whose availability falls into one of these orderability groups; combined with `availability`, a listing must satisfy both.";
pub const INCLUDE_UNSPECIFIED_AVAILABILITY: &str =
    "Also include listings that state no availability (default false).";
pub const CREATED: &str = "Range on the listing's creation time, as ISO 8601 date-times with offset; both bounds inclusive.";
pub const UPDATED: &str = "Range on the listing's last update time, as ISO 8601 date-times with offset; both bounds inclusive.";
pub const LOT_BIDDING_OPENS: &str = "Range on the time bidding on the lot opens, as ISO 8601 date-times with offset; `min` inclusive, `max` exclusive.";
pub const LOT_SCHEDULED_CLOSES: &str = "Range on the lot's scheduled closing time, as ISO 8601 date-times with offset; `min` inclusive, `max` exclusive.";
pub const DATE_RANGE_MIN: &str = "Start of the range, as an ISO 8601 date-time with offset.";
pub const DATE_RANGE_MAX: &str = "End of the range, as an ISO 8601 date-time with offset.";
pub const SEARCH_LANGUAGE: &str = "Language of the returned listing text and of the titles `productQuery` is matched against, as an ISO 639-1 code (default `en`).";
pub const SEARCH_CURRENCY: &str = "Currency prices are returned in and `price` is interpreted in, as an ISO 4217 code (default `EUR`).";
pub const LANGUAGE: &str =
    "Language of the returned listing text, as an ISO 639-1 code (default `en`).";
pub const CURRENCY: &str = "Currency of the returned prices, as an ISO 4217 code (default `EUR`).";
pub const PAGE_SIZE: &str = "Number of results per page, from 1 to 100 (default 21); values outside that range are clamped.";
pub const GEOGRAPHY: &str = "Where listings are located, by country, subdivision or distance from a point; every member given must hold.";
pub const SHIPPING: &str = "Only listings whose advertised shipping terms meet these conditions for a destination country; every member given must hold.";
pub const SHIPPING_MAXIMUM_PER_ITEM: &str = concat!(
    "Highest acceptable shipping price per item, in the amount's own currency; requires a `destination` country. ",
    minor_unit_rule!()
);
