# Geography contract

`geo` owns geographic representation and comparison. Its default features expose
pure values and local reference lookups; OpenSearch and Serde compatibility
modules require explicit features. Geocoding clients live in separate adapter crates.
The reused `isocountry` dependency retains its existing Serde support; new boundary
code uses the strict release-aware parser instead
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

The unused Google geocoder and legacy geocoding service are removed, along with their
`google`/`service` features and HTTP/runtime/mock dependencies in `geo`. New consumers
use the provider-neutral `geo-service` capability. OpenSearch distance helpers require
`opensearch`; `full` enables the remaining compatibility features. Distance struct
literals must use the validated constructor and accessors.
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
Party-location management grant or membership in that Party's active partnership.
Both paths require an unsuspended user and permit management and private reading of
all locations of that Party, without conferring Party editing privileges. Application
approval establishes membership, so existing and later-added members obtain location
authority without a separate grant or backfill. Removing membership or dissolving the
partnership removes that derived authority. Independent explicit grants survive those
changes; only administrators/trusted internal callers may grant or revoke them, and
revoking an explicit grant does not override active partnership membership.

Delegated credentials additionally need existing `parties:write` for mutations and
`parties:read` for protected reads. These scopes alone confer no management authority;
`product-listings:write` confers neither location access nor mutation rights. Location
operations, explicit grant changes, partnership membership changes and partnership
creation/reactivation/dissolution serialize on the owner Party row before changing
partnership, membership or location state.

REST and application use cases expose only protected location management and
administrator grant/revoke operations. Get/list always enforce management authority.
Membership must belong to the requested Party's active partnership. There are no public
Party-location get/list endpoints or application use cases. Management reads include
private and retired sites, with their evidence and revision tokens.

Creation defaults to PRIVATE. The persisted disclosure policy governs future public
geography consumers, which must exclude PRIVATE and retired locations.
COARSE_PUBLIC permits only roles and asserted country/subdivision; it excludes label,
free-text address, postal text and every coordinate, even with claimed coarse precision.
EXACT_PUBLIC permits label, address/postal assertions and coordinates. Evidence and
protected revision tokens must be excluded from every public view. A ListingSource
reference cannot make a location less private.

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
Lists are live traversals; management authority is reevaluated each page.

## Geocoding capability

`geo-service` owns the provider-neutral geocoding port and use case; `geo` remains
the pure geography owner. A request needs only validated, unchanged address text.
Candidate limits have a bounded default; optional submitted assertions, independent
hard country/subdivision constraints, a position ranking hint and presentation language
refine the query. Assertions are never appended to the query or converted into filters
implicitly. Hints are neither verified constraints nor canonical facts.

The consuming Party, User or other service owns authorization, privacy purpose, user
identity, acceptance and persistence/cache scope. The geocoding capability receives
no dealer/user classification or user identifier and performs no persistence or caching.
Provider retention is configured once when the composition root constructs the adapter;
every response reports whether its derived evidence permits storage.

An explicit empty result collection is no match. Multiple candidates are ambiguity;
a single returned candidate is evidence, not proof of uniqueness or an acceptance
decision, especially when the requested limit truncates other possible matches.
Unsupported components, inconsistent components, assertion/constraint mismatches,
unverifiable constraints and technical failures remain distinct application outcomes.
Country-only evidence can be usable without normalized subdivision or point. Provider
scores describe token matching; they are not probabilities and there is no automatic
score threshold. The AWS SDK defaults an omitted/null overall score to zero, so this
adapter conservatively treats zero as unavailable too; it never invents a supplied
score from that default. A consuming service owns any calibrated acceptance policy.

The Amazon Location adapter uses the
[Places v2 Geocode API](https://docs.aws.amazon.com/location/latest/APIReference/API_geoplaces_Geocode.html)
with SigV4 authentication. The composition root injects its SDK client and credentials;
the adapter pins the reviewed Frankfurt endpoint/region, bounds concurrent calls and
deadlines, and uses standard
SDK transient/throttling retries. Provider query limits are checked before transmission,
without truncation. Unknown vocabulary never becomes invented core precision. Point
addresses map to premises; interpolated addresses map to street evidence; explicitly
estimated/inferred addresses and POIs use broader representative evidence. No accuracy
is inferred. Provider country codes use the pinned ISO release; conflicting alpha-2/
alpha-3 components invalidate combined geography. AWS `[longitude, latitude]` is mapped
explicitly to G1 accessors, and invalid coordinates fail the whole response.

AWS [Region.Code](https://docs.aws.amazon.com/location/latest/APIReference/API_geoplaces_Region.html)
is an abbreviation rather than an ISO guarantee. The approved compatibility mapping
is the documented Canadian `BC` example to validated `CA-BC`; other region/subregion
codes and names are unsupported evidence, never guessed by prefixing a country.
More mappings require reviewed provider meaning and pinned ISO validation.
External PlaceIds remain provider references, never Aura identifiers.
The returned interpretation profile is immutable; changes to mapping meaning or
applicable provider terms require a new profile and deliberate retained-cache invalidation.

### Provider terms and retention gate

The implementation's terms profile is **Amazon Location Places v2 / default data /
eu-central-1 / October 2026**. V2 selects service default data rather than a caller-created
place index. The response does not identify individual upstream suppliers; provenance
records `AMAZON_LOCATION_DEFAULT`, not an invented supplier assertion. AWS's
[attribution page](https://docs.aws.amazon.com/location/latest/developerguide/data-attribution.html)
lists the underlying sources. Before enabling retention, operators must establish the
actual applicable data-source terms for this product/region/account and confirm the
proposed display and reuse. The adapter conservatively applies the HERE Japan storage
prohibition to all default
data. Other providers, products and regions require a separate profile and review.

[AWS Service Terms section 82](https://aws.amazon.com/service-terms/#82._Amazon_Location_Service)
permits geocode retention when declared through the API, subject to provider restrictions;
HERE Japan data cannot be stored or cached. It restricts systematic collection,
competing location services lacking independent value, atlases, advertising/marks in
Location Data and redistribution under incompatible open-data obligations. Ordinary
reuse inside Aura's independently valuable dealer/listing service requires review;
this contract grants no raw-data resale, bulk export, general public geocoding cache or
map-display rights. Requests can be processed by suppliers outside the AWS region;
region selection alone is not a privacy/data-residency guarantee. Private address use
requires a purpose-specific privacy review and user isolation.

The [intended-use contract](https://docs.aws.amazon.com/location/latest/developerguide/places-intended-use.html)
requires `IntendedUse=Storage` for retained results, including caching. Every retained
request explicitly uses that API value. The
[pricing categories](https://docs.aws.amazon.com/location/latest/developerguide/places-pricing.html)
are **Core** for our single-use requests without additional features, and **Stored**
for retained requests; the API enum is `Storage`, not the category name `Stored`.
Storage supports indefinite provider retention where otherwise permitted, not a waiver
of source-specific restrictions or personal-data erasure requirements. Rates are
region-dependent: review [current pricing](https://aws.amazon.com/location/pricing/)
before authorizing paid evaluation or production volume.

The adapter defaults to `ResultRetention::SingleUse`. A composition root enables
retained results with `AmazonLocationConfig::default().with_retention(ResultRetention::Storage)`;
the same address-only request works with either adapter configuration. This option
controls provider request intent and pricing, without recording an operator review or
business purpose in code. A hard country constraint is optional. Japan-constrained
storage requests fail before network access; returned Japanese, missing or contradictory
countries fail before retained evidence is released. Other constraint mismatches remain
candidate outcomes and never establish an acceptance decision.

Any future persistence/cache owner must check `result.retention.permits_storage()`
before writing **any** derived components, including country/subdivision/coordinates,
and preserve reference release, provenance, attribution and interpretation profile.
Single-use evidence cannot be cached or promoted into retained business data. Derive
private ownership from authenticated context and isolate private results to that user;
private evidence must never enter shared dealer caches or cross-user caches. Business
services own those checks and cache keys, including query, constraints/hints/language
and mapping/profile version. Storage permission does not grant authorization, satisfy
personal-data rules or accept a candidate. Disable retained resolution and reuse when
applicable provider rights cease to apply.

Pass through attribution when showing derived data to others: conspicuously link AWS's
attribution page in end-user terms/product documentation and retain required supplier
notices. Maps have additional display obligations; there is no map UI in this capability.
Logs must exclude queries, user coordinates, identities, PlaceIds, credentials, provider
messages and payloads. Report only static failure classifications/counts; ordinary
Display/Debug on the new port is redacted. Original SDK causes remain accessible for
controlled diagnosis and must not be dumped into logs. SDK HTTP/body tracing must
remain disabled.

### Offline evidence and controlled quality evaluation

The [fixture set](../src/geo-amazon-location/fixtures/geocode.json) is synthetic,
shaped against the documented API, and exercises the real SDK over an isolated local
server with test credentials. It covers DE/GB/FR/IT/US/CA/AU examples, ambiguity,
Japanese/Arabic multiline text, country-only input, Puerto Rico, omitted components,
approximate/estimated positions and future precision vocabulary. Additional tests cover
coordinate order, nonfinite/out-of-range values, malformed successes, constraints,
adapter retention intent, storage restrictions, deadlines, authentication and retry
exhaustion. This establishes protocol/normalization behavior, **not measured market
coverage or worldwide accuracy**.

Amazon Location is selected initially because typed candidate geography, explicit storage
intent and SigV4 fit this AWS backend without provisioned place indexes. The conservative
subdivision mapping and Japan retention restriction are known limitations. No live
benchmark, account resource or paid provider call is part of CI or this implementation.

For a separately authorized manual evaluation:

1. Record an approved account/region, provider/source terms, call/cost cap, request intent
   and attribution/privacy requirements. Use public dealer examples or consented private
   data; never live user records as benchmark fixtures.
2. Build a small labeled sample in actual target markets, including partial/ambiguous,
   non-Latin, territory and approximate cases. Establish expected country, subdivision,
   coordinate vicinity and useful precision independently of this provider.
3. Wire the adapter through the
   [manual composition example](../src/geo-amazon-location/examples/geocode.rs).
   With approved test-account credentials, `cargo run --locked -p geo-amazon-location --example geocode`
   reads a public dealer query from stdin and makes a **paid-capable** single-use request.
   It prints only safe counts/classifications and persists no results. The example is not
   run in automated validation. Retained evaluation requires the adapter storage option
   and compliance with the applicable provider conditions.
4. Evaluate candidate recall, ambiguity, country errors, spatial error against the
   independent labels, useful precision and subdivision mapping gaps. Track error rates
   and latency by market/input class; do not reinterpret match scores as confidence.
   Keep only rights-permitted evidence in the approved review record and report aggregate
   statistics otherwise. Record date, source/profile, limitations and acceptance criteria.
5. If coverage or rights are inadequate, implement another reviewed adapter behind the
   same port. Never silently substitute Google or public Nominatim. Review sample-derived
   acceptance rules separately before using results to mutate business state.

No current business rows or public REST contracts change.
