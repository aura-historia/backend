# Shared public contracts

REST and MCP import the same DTO definitions. Move a family's existing Serde types,
codecs and pure view-to-DTO conversions here when its operation is exposed through
another adapter, then replace the REST definitions with imports. Preserve field
names, validation, aliases, prices, patch semantics and pagination. This crate may
use domain values and service views for pure conversions; dependencies never point
back from those layers to wire contracts.

HTTP extraction, status codes, headers, cache policy, `ApiError` and `IntoResponse`
stay in REST. MCP may wrap existing path/query/body values or an HTTP-only outcome;
its nested business fields use shared DTOs. Do not declare a second MCP model or
translate OpenAPI into a separately maintained schema model.

For example, a family's shared patch DTO can reuse the common price directly:

```rust
use aura_historia_contracts::{patch_value::PatchValue, values::PriceData};
use schemars::{JsonSchema, generate::SchemaSettings};
use serde::Deserialize;

#[derive(Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct UpdateExample {
    #[serde(default)]
    price: PatchValue<PriceData>,
}

// REST deserializes this same type. MCP uses its accepted-input schema.
let schema = SchemaSettings::default()
    .for_deserialize()
    .into_generator()
    .into_root_schema_for::<UpdateExample>();
let omitted: UpdateExample = serde_json::from_str("{}")?;
assert!(matches!(omitted.price, PatchValue::Omitted));
# Ok::<(), serde_json::Error>(())
```

`PatchValue<T>` models omission, explicit null and a concrete value; `#[serde(default)]`
on the enclosing member preserves omission as `Omitted`. Without that attribute,
Serde accepts a missing member as `Null`; its schema permits omission in either case
and describes a present member as null or `T`. It is an input representation and does
not serialize a fictitious `Omitted` JSON value. Family-specific restrictions such
as rejecting null must also be reflected in that family's schema and validation.

Generate input schemas with `SchemaSettings::for_deserialize` and output schemas
with `SchemaSettings::for_serialize`. Localized text accepts existing locale aliases
on input and emits canonical ISO language codes. Currency codes and unsigned minor
unit amounts retain the existing public representation. The existing `ON_REQUEST`
unit variant accepts unknown members on input despite Serde's enum-level
`deny_unknown_fields`; its schema preserves that compatibility behavior.

Test actual accepted and rejected JSON against the generated schema and actual
serialized values against the output schema. Reuse the small
[common-value/patch fixture](tests/fixtures/common-values.json) as illustrated in
[the schema tests](tests/schema.rs) and [REST consumption](../aura-historia-api/src/values.rs).
Custom serializers need an explicit schema override when derive cannot describe
what they accept or emit. Stored representations, including `shipping-json`, are
separate contracts and must not change as a side effect of public DTO extraction.

Listing detail, summary, presentation and search contracts live under `product_listings`.
The listing MCP family assembles these existing values in thin object arguments; REST retains
query/path extraction and its HTTP-only pending, caching and error representations. Geography
and shipping filters and public shipping serializers are shared as well. Shipping terms retain
the `shipping-json` codec's own DTO/schema owner and adapter-selected reference representation.
The [listing fixtures](tests/listings.rs) check full filters, exact unsigned values, required nulls
and public shipping redaction alongside the REST and MCP adapter fixtures.

Saved-search CRUD, patches, match feedback and match continuation live under `saved_searches`.
REST and MCP reuse the same DTOs, validation and service-view conversions; REST alone query-encodes
match cursor arrays. The [saved-search fixture and schema tests](tests/saved_searches.rs) cover
full listing criteria, patch omission/null rules, atomic availability updates and stored match
shipping valuations. Account identity comes from each trusted transport context, never these DTOs.

Watchlist entry, mutation, list query and paged personalized-detail contracts live in `watchlist`.
REST and MCP share their constructor-based ID validation, notification defaults, nonnullable
patch validation and timestamp/listing-ID cursor codecs. The [watchlist contract tests](tests/watchlist.rs)
compare schema validation with the complete input mapping, including null and omission rules.

Shipping profile/default contracts live under `shipping::management`; `shipping::imports` owns the
provenance DTO also re-exported by asynchronous listing ingestion. `shipping::terms::schema` exposes
the complete authoritative terms schema with public references for other operation families.
[Shipping fixtures](tests/shipping_management.rs) compare both schema directions with actual codecs,
including public TypeIDs versus persisted UUIDs, required nulls, complete terms and import metadata.

Protected party-location management DTOs and pure command/view mappings live in
`party_locations::management`; source-default references and applicability evidence live in
`listing_sources::location_default`. REST and MCP preserve their full fields, revision codecs,
patch null rules and bounded location pages. Management `GeographyData`/`PositionData` remain
separate from public `GeographicSearchData`. The [location contract tests](tests/location_management.rs)
check actual JSON against schemas, including nullable clears and retained evidence.
