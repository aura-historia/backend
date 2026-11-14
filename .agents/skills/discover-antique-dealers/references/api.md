# Onboarding API notes

Recheck `docs/swagger.yaml`, `docs/geography.md` and `docs/shipping.md` if contracts change.
Production: `https://api.aura-historia.com` (the only deployed stage). Credentials
need `parties:read/write` and `listing-sources:read/write`. Never send them to dealers.

`scripts/api.py snapshot` follows all admin Party/source pages. Build a run-local domain
index and use filtered admin queries for fresh identity checks at larger volumes. Source
summaries put URLs under `presentation`; names/contacts are substring searches, not unique.

- `POST /v1/admin/listing-sources`: required `name`, `listingSourceType`, `operator`,
  `ingestionConfiguration`. Discovered dealers, including a dealer's own storefront on a
  marketplace, are `DEALER`.
  `operator={"type":"NEW","name":"operator","email":"…","phone":"…"}` creates a Party
  atomically; alternatively `{"type":"EXISTING","partyId":"pty_…"}`. `url` is the
  actual dealer catalogue; `image` is an inspected logo. Optional `platformTechnology`
  is descriptive SCREAMING_SNAKE_CASE, independent of ingestion. Always supply
  `ingestionConfiguration: []` during discovery, even for an inspectable catalogue.
  Read `GET /v1/admin/listing-sources/{id}` for the operator Party ID.
  Organizations are Parties; there is no separate organization create route.
  Separate own storefronts with different catalogues/policies may share an operator.
  A reviewed row can declare `existingOperator={ownerDomain,partyId,partyLocationIds,
  confidence,evidence}`. The owner must already be `ADDED` in that same ledger. The
  command checks the live Party and reused locations before creating a source with
  `operator.type=EXISTING`; it creates no replacement Party or duplicate locations.
  Keep the existing Party's primary name/email/phone in the row, retaining additional
  shop contacts as separate evidence. Reused location IDs correspond to the first
  entries of the row's reviewed locations. This does not establish stock or pickup.
- `PATCH /v1/admin/listing-sources/{id}` with `{"ingestionConfiguration":[]}` clears
  existing ingestion methods without changing platform metadata. `platformTechnology`
  and `image` can be patched independently. Omitted members remain unchanged; image-only
  enrichment uses `{"image":"https://…"}`. Read back metadata and empty ingestion after
  updates, preserving the source and operator IDs.
- `POST /v1/parties/{id}/locations`: `label`, explicit `roles`, optional `geography`
  with `referenceRelease=iso-codes-4.20.1`, `addressText`, `country`, `postalCode`.
  Evidence is `{reference: URL,observedAt: RFC3339,scope: SITE_DESCRIPTION |
  REGISTERED_ADDRESS | CORRESPONDENCE_ADDRESS}`. List before retrying uncertain POSTs;
  locations have no creation key. Do not infer visitor or inventory availability.
  Location roles are `BUSINESS_PREMISES`, `WAREHOUSE`, `REGISTERED_ADDRESS` and
  `CORRESPONDENCE`; the last differs from evidence scope `CORRESPONDENCE_ADDRESS`.
  Roles form a set and the API returns canonical order. Compare membership and
  uniqueness, rather than input array order; retain the returned ID before validation.
  Omit unknown optional geography members instead of sending explicit null: reads
  omit them. Reconcile a representation mismatch against saved location IDs before
  resuming; never recreate a location merely because an unknown field disappeared.
- `GET` then `PUT /v1/admin/listing-sources/{id}/location-default`: current
  `expectedRevision`, `partyLocationId`, optional
  `businessAssociation={reference: URL,scope: ASSOCIATED_BUSINESS}`, and separately
  `advertisedInventory={reference: URL,scope: ADVERTISED_INVENTORY}`. Legal addresses
  alone establish no inventory assertion. When the operator Party has exactly one active
  location, the `DEALER` source asserts inventory there with reference
  `aura-historia:policy/single-location-dealer-inventory/v1`, unless first-party evidence
  supplies its own reference (`docs/geography.md`). Multi-site dealers need explicit
  evidence. `PATCH` on the same path changes only applicability: omitted keeps, `null`
  removes; it requires the current `expectedRevision` and an active reference.
- `POST /v1/listing-sources/{id}/shipping-profiles`: stable `creationKey`, `content`
  (`name`, required nullable `attributedPartyId`, `terms`) and `evidence` entries
  `{reference: URL,assertion: short description}`. Reconcile uncertain creation with
  the same key and original body, never a new key. Read the returned profile.
- `PUT /v1/listing-sources/{id}/shipping-profiles/{profileId}` replaces complete
  `content` and `evidence` using the profile's `managementRevision` as
  `expectedRevision`. Preserve its ID and unrelated terms. Reads omit protected
  evidence, so reconstruct your own reviewed evidence references from the ledger;
  do not silently erase provenance. Journal intent and verify the returned content,
  revisions and selected default. Definite rejected input can be corrected only
  after confirming no resource was created; uncertain writes require reconciliation.
- `GET` then `PUT /v1/listing-sources/{id}/shipping-default`: current `expectedRevision`
  and returned `shippingProfileId`. Read both defaults back and check active eligibility.

Shipping uses strict `SHIP_TERMS_V1`: snake_case inside `terms`, camelCase outside.
Every nullable member is required, including `shipping/collection`, applicability's
`class/variants/quantity/threshold`, `dispatch_origin`, service's `rate/arranger/performer`
and all three `lead_times` members. Read schemas instead of guessing. ISO codes must
belong to the named release.
When a time range is present, `minimum` and `maximum` are required numeric bounds;
only its `basis` and `trigger` may be null. “Within three working days” can use
`minimum: 0, maximum: 3, unit: DAYS, basis: BUSINESS`; omit the whole time component
when no faithful range is known.

Use `PARTIAL` coverage when destinations are incomplete; leave rates null when costs
are incomplete. Free-shipping thresholds belong on the free service's applicability with a
fallback unknown-price service preserving availability when the basket is unknown.
An explicitly advertised lower bound such as “from EUR 20” can use
`price={"kind":"FROM","amount":{"amount_minor":2000,"currency":"EUR"}}`;
it does not establish a fixed EUR 20 fee or a particular charging unit.
Do not invent shipping classes without real per-item crawler assignments. An explicit
weight tariff does not establish an on-request price. Applicability `quantity` counts
units; never put gram/kilogram bounds there. Retain weight brackets in the ledger until
actual item classes can represent them. Whole-basket article-count tariffs also need
their actual seller-order count: applicability quantity has no basket scope. Preserve
those brackets in the ledger instead of treating one listing's quantity as the basket.
`ON_REQUEST`
needs an explicit advertised price mode. `ANYWHERE` requires explicit worldwide terms,
not just “international”, including for `SUBJECT_TO_CONFIRMATION` services. Use an
`UNRESOLVED` area when the foreign countries are unspecified. Null collection is unknown; explicit no-pickup may assert
`NOT_OFFERED`.

Geographic completeness is independent of price completeness. An explicit exhaustive
destination policy such as “only Germany and Austria” supports `COMPLETE` coverage
of those countries even when rates remain null. A country selector does not.

Service `eligibility` is also required-nullable. Verified destination coverage can
be retained with `eligibility: null` when item-size/class inputs prevent a faithful
source-wide offer assertion. This preserves geographic evidence without inventing
unconditional or universally confirmation-dependent delivery. Keep item-specific
freight requirements in the ledger until actual class assignments exist.

Service features form a unique set and the API returns its canonical order. Compare
feature membership without ignoring duplicate values; preserve geographic zone order,
service ordering, applicability and every other term. An order-only feature difference
does not require recreating a profile. Resume using its journaled resource ID.

Leave `rate.basis` null unless the dealer identifies the charging unit. A listed fixed
amount or quote request alone does not establish per-order or per-item pricing.

```sh
python3 .agents/skills/discover-antique-dealers/scripts/api.py snapshot
python3 .agents/skills/discover-antique-dealers/scripts/api.py audit data/dealer-discovery/countries/DE/dealers.jsonl
```

Supply `AURA_HISTORIA_ACCESS_TOKEN` or `--credential-file` with an external owner-only
file. Never inline credentials in commands. The client refuses redirects/arbitrary origins
and does not retry writes. Error output deliberately omits server bodies and headers.

Delivery availability and pricing are independent: a price quote does not make an
offer confirmation-dependent, and delivery available on inquiry does not by itself
assert an on-request price. Zones are ordered: a more specific advertised exception
can precede a broader zone. Earlier unresolved geography blocks later positive matches.
