# Geography contract

`geo` owns geographic representation and comparison. Its default features expose
pure values and local reference lookups; Google, OpenSearch and Serde compatibility
modules require explicit features. The reused `isocountry` dependency retains its
existing Serde support; new boundary code uses the strict release-aware parser instead
of its permissive parsing. Shipping owns destination selectors, exclusions,
precedence and delivery assessment. A free-text address never establishes a
shipping postcode: supply destination country/subdivision/postal assertions explicitly.

## Assertions and evidence

Descriptions may contain any subset of address text, country, subdivision and
postal text. All absent fields produce absence; present blank, oversized or invalid
fields fail. A subdivision must belong to an explicitly asserted country when both
are supplied. Subdivision-only evidence retains its country association without
inventing a submitted country assertion. Missing facts use `Option`; update commands
must represent unchanged/set/clear intent separately from semantic absence.

Submitted geography and derived geography have distinct core types. Derived evidence
must not silently replace submitted assertions. The owning adapter retains the
original source observation separately, including unsupported provider codes and
unparsed components; provider payloads, audit times and processing state are not core
geography. Provider region codes and province abbreviations do not establish ISO
subdivisions. Compatibility mappings require explicit approval and documentation.

Address and postal text preserve Unicode, punctuation, leading zeros and surrounding
spacing byte for byte. Bounds are explicit UTF-8 byte limits in their constructors;
oversized input fails without truncation. Address text permits LF and CRLF line
breaks; other control characters fail. Postal text is single-line. Comparison keys
do not change stored observations. The current conservative rules fold ASCII case
and one conventional separator space for recognizable GB/CA postal shapes; unsupported
countries, unfamiliar shapes and unfamiliar spacing retain exact text. Recognition
and lookup absence provide no guarantee of postal existence, invalidity or deliverability.
The supported shapes follow the [GOV.UK postcode format guidance](https://assets.publishing.service.gov.uk/government/uploads/system/uploads/attachment_data/file/257329/additional-validation.pdf)
and [Canada Post's PCCF reference](https://www.canadapost-postescanada.ca/cpc/assets/cpc/uploads/files/marketing/2017-postal-code-conversion-file-reference-guide-en.pdf),
including Canada's letter exclusions and the exact `GIR 0AA` exception.

Positions are WGS 84 degrees with explicit latitude-then-longitude accessors. Inputs
and rehydration reject nonfinite and out-of-range coordinates and nonfinite or negative
distances. Distance units are explicit. `(0, 0)` is valid evidence. Premises, street,
locality and broader representative precision are asserted explicitly; optional
accuracy is nonnegative metres and is never inferred from decimal places.

## Reference release and interpretation

The chosen reference is Debian **iso-codes 4.20.1**, tag `v4.20.1`, commit
`74623235ae99f6e835e1e465017ce4c544ae6b53` from
[the upstream repository](https://salsa.debian.org/iso-codes-team/iso-codes/-/tree/v4.20.1).
Its ISO 3166-1, 3166-2 and 3166-3 JSON source files are vendored unchanged under
[`src/geo/reference/iso-codes-4.20.1`](../src/geo/reference/iso-codes-4.20.1).
Upstream's `REUSE.toml` attributes these data to **2016 Dr. Tobias Quathamer
<toddy@debian.org>**, under **LGPL-2.1-or-later**. The attribution file and full license
are included beside the data, with annotation paths adapted to this directory;
generated lookup tables retain that attribution and
license. Redistribute those notices, license and corresponding data source with
the tables; modifications to the licensed data must remain available under those terms.

Application validation uses the complete vendored code lists without network access.
`CountryCode` reuses `isocountry`; release tests require its entire code set to agree
with the chosen data. Boundary parsing accepts only exact uppercase alpha-2 country
codes and complete ISO subdivision codes present in the named release. It never
guesses aliases, treats EU/customs zones as countries, or accepts private/provider
codes merely because their syntax resembles ISO.

Persist and rehydrate the reference release alongside codes. Unknown releases fail;
never substitute the current release. Existing releases and their interpretations
are immutable and must remain supported after updates. ISO 3166-3 alpha-4 lookup
keeps recognized withdrawn countries interpretable, including reused alpha-2 codes;
it does not turn them into current destinations or infer historical geography.
Withdrawn/changed codes are not automatically remapped to successors.
Descriptions retain the explicit release even for country-only assertions; all codes
in a description must use that release. Standalone country identifiers require their
original release at the owning persistence boundary because the reused country enum
does not carry release metadata.

To reproduce the tables offline, run:

```sh
python3 src/geo/scripts/generate_reference.py
python3 src/geo/scripts/generate_reference.py --check
```

The generator checks pinned source hashes and emits deterministic Rust tables using
Python's standard library and `rustfmt`. To acquire the same source, check out the
upstream commit above and compare the three `data/iso_3166-*.json` files to the
vendored copies. To adopt a new release, add a separate immutable snapshot, provenance,
hashes and explicit release dispatch; retain existing decoding and historical meaning,
review additions/removals/reassignments and run the full reference/codec tests. A changed
code's meaning requires explicit migration or versioned interpretation, never a moving
`latest` lookup. Do not replace the old snapshot or blindly update `isocountry`.
CI checks pinned source hashes and exact generated output without modifying files.

## Derived continent classification

`Continent::country_grouping` implements named **Aura country grouping v1**, a coarse
country/territory classification rather than position-derived physical geography or
an ISO field. It returns no grouping for known transcontinental or dispersed territorial
cases where a single classification would be misleading. The uncertainty list lives
in code and covers, for example, Russia, Egypt, France and US outlying islands.
Remaining territory codes are grouped independently of their administering state.
The method does not locate a particular address on a physical continent. Continental
boundary conventions remain coarse; precise geography requires separate position
evidence. Continent is derived for reads, never writable address state. EU membership,
customs zones and delivery regions belong to their own policy vocabularies.

## Compatibility and boundary migration

Party locations consume these pure values; their transport and PostgreSQL adapters
validate through the constructors and retain the asserted reference release.
The former `StructuredAddress` carrier and its
unused OpenSearch reconstruction helper are removed. `AddressText` owns free-form
address observations; `GeographicDescription` owns partial validated assertions.
Source components remain evidence at their owning adapter and are not guessed
into ISO subdivisions. Continent is derived from country assertions when unambiguous.

Both Google interfaces now accept validated `AddressText`, preserving its exact text
through the documented [unstructured address query parameter](https://developers.google.com/maps/documentation/geocoding/reference/rest/v4/geocode.address/geocodeAddress).
Empty/invalid input fails during construction, before a provider request. Geocoder
responses remain separate provider-derived evidence. Google interfaces require
`google`/`service`; OpenSearch distance helpers require `opensearch`; `full` enables all
compatibility features. Distance struct literals must use the validated constructor
and accessors.
The optional `data` codecs retain canonical code strings and explicit release/units;
conversion into domain values always calls constructors. These are compatibility
codecs, not new REST shapes or business database schemas. Future persistence and
transport owners must use similarly fallible mappings and keep submitted/derived
fields and patch intent explicit.

## Party locations (#1999)

A PartyLocation is one Party's independently identified association with a site, not
a canonical address, PartyContact, listing inventory fact or global default. Different
Parties at the same address keep different IDs and permissions. Roles describe business
use only; a warehouse role grants no claim about stock, dispatch or collection. Partial
descriptions, free text, country-only assertions and entirely unresolved sites are valid.
Creation does not contact evidence URLs, geocoders or an LLM. Coordinates accepted here
are caller assertions with explicit precision; derived resolution is future enrichment.

Correction explicitly asserts the same physical site and retains the ID. Relocation is
an explicit create command referencing the old ID and its expected revision; it creates
an independent ID and retires the old location atomically. Retirement preserves history,
blocks new assignments and excludes the old site from current public/inherited geography.
Restore is explicit and never reassigns consumers. Retained locations and grants block
Party deletion. No location operation automatically selects a ListingSource default.
Cross-context consumers must check active eligibility; dependency invalidation belongs
to subsequent roadmap issues.

### Authorization and disclosure

Use cases accept the existing trusted OperationContext. Services/system callers and
verified administrators may manage locations. Other users need an explicit, revocable
Party-location management grant. Only administrators/trusted internal callers may grant
or revoke it. It permits management and private reading of that Party's locations,
without conferring Party editing, ListingSource or listing privileges. Delegated
credentials additionally need existing `parties:write` for mutations and `parties:read`
for protected reads. These scopes alone confer no management grant;
`product-listings:write` confers neither location access nor mutation rights. Revocation
and protected operations serialize on the owner Party row.

Creation defaults to PRIVATE. Public readers exclude PRIVATE and retired locations.
COARSE_PUBLIC reveals only roles and asserted country/subdivision; it omits label,
free-text address, postal text and every coordinate, even with claimed coarse precision.
EXACT_PUBLIC permits label, address/postal assertions and coordinates. Evidence and
protected revision tokens are excluded from every public response. A ListingSource
reference cannot make a location less private. Public endpoints allow anonymous access
and validate supplied bearer credentials; authenticated callers still receive only
public-safe fields. Invalid supplied credentials are rejected rather than downgraded.

Caller evidence contains an opaque bounded reference, optional observation time and
one declared assertion scope: site description, registered address or correspondence
address. It establishes neither listing storage/pickup nor caller identity/trust. Actor
identity comes from OperationContext; evidence cannot override it. Evidence and address
content are excluded from structured logs. Private state and creation receipts must
remain outside CDC/public projections. Only the minimal change signal is intended for
later invalidation routing; no downstream invalidation route is activated here.

### Retry, revision and persistence guarantees

The idempotency namespace is `(Party, trusted principal kind/identity, Idempotency-Key)`.
Keys are 1–128 ASCII letters/digits or `-_.:`. Completed creation semantics and its original
result commit with the location and any relocation. Independent retries serialize on the
Party row and replay that original result, even after later correction/retirement; changed
semantics under the same key fail with conflict. Address equality does not establish
identity. Receipts have no expiry; retries must retain the same trusted caller identity.

Protected results expose a positive expected-revision token for this explicit CAS
contract. Every update/lifecycle command submits it; stale revisions conflict even for
identical requests. This is the deliberate location-specific exception to the ordinary
use-case preference to hide storage versions in `arch.md`. Omitted PATCH members are
unchanged, nullable members clear on null, and required members reject null. Roles are
replaced with an explicit set; an empty set clears them. Identical
commands at the current revision do not advance revisions, update timestamps or emit a
semantic change signal. Changed writes advance authoritative revision once; exhausted
revisions reject changes with conflict while permitting same-state no-ops. Geography,
asserted position or evidence changes also advance input revision; label/roles/disclosure
changes and lifecycle transitions do not rewrite inputs. Future derived resolution must
pin this input revision and ignore mismatched results.

`party_location_changes` is an append-only revision signal committed with semantic writes,
keyed by location ID/revision. It contains only owner/location identities, revision, input
revision, lifecycle, trusted actor and occurrence time. Future consumers must process it
idempotently and check authoritative eligibility. It is not an event-sourced aggregate
or a second business model. Exact constraints belong to migrations/adapters; public
shapes and errors belong to [OpenAPI](swagger.yaml). Lists use ascending native UUID
keyset order, bounded page sizes and `ploc_` continuation IDs, without listing hydration.
Lists are live traversals; disclosure/lifecycle eligibility are reevaluated each page.
