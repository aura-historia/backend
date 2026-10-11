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
are caller assertions with explicit precision; [durable derived resolution](#durable-resolution-and-party-integration-2002)
is separate enrichment.

Correction explicitly asserts the same physical site and retains the ID. Relocation is
an explicit create command referencing the old ID and its expected revision; it creates
an independent ID and retires the old location atomically. Retirement preserves history,
blocks new assignments and excludes the old site from current public/inherited geography.
Restore is explicit and never reassigns consumers. Retained locations and grants block
Party deletion. No location operation automatically selects a ListingSource default.
Cross-context consumers must check active eligibility; listing dependency invalidation
uses the [effective geography contract](#effective-listing-geography-2003).

### Authorization and public geography

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
active and retired sites, with their evidence and revision tokens.

Active PartyLocations always permit exact public geography; there is no disclosure
setting. Retired locations are ineligible. Public consumers still honor explicit source
applicability and current provider-retention eligibility. Evidence and protected revision
tokens stay outside public views. Listing documents contain only bounded derived fields,
never raw address/evidence/provider payloads.

Caller evidence contains an opaque bounded reference, optional observation time and
one declared assertion scope: site description, registered address or correspondence
address. It establishes neither listing storage/pickup nor caller identity/trust. Actor
identity comes from OperationContext; evidence cannot override it. Evidence and address
content are excluded from structured logs. Protected location state and creation receipts
remain outside CDC/public projections. The minimal change signal establishes listing
dependency invalidation without publishing location content.

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
asserted position or evidence changes also advance input revision; label/role
changes and lifecycle transitions do not rewrite inputs. Derived resolution must
pin this input revision and ignore mismatched results.

`party_location_changes` is an append-only revision signal committed with semantic writes,
keyed by location ID/revision. It contains only owner/location identities, revision, input
revision, lifecycle, trusted actor and occurrence time. Consumers must process it
idempotently and check authoritative eligibility. It is not an event-sourced aggregate
or a second business model. Exact constraints belong to migrations/adapters; public
shapes and errors belong to [OpenAPI](swagger.yaml). Lists use ascending native UUID
keyset order, bounded page sizes and `ploc_` continuation IDs, without listing hydration.
Lists are live traversals; management authority is reevaluated each page.

## ListingSource location defaults (#2000)

Each source owns zero or one explicit operator-owned PartyLocation reference. Sources of
one Party may select different sites; there is no Party-level default or automatic
first/nearest/only-site selection. Applicability has independent optional
ASSOCIATED_BUSINESS and ADVERTISED_INVENTORY assertions on that same reference. Each
requires bounded caller-supplied evidence explicitly scoped to its meaning and source.
Legal/correspondence addresses and PartyLocation roles never establish inventory.
Neither meaning establishes seller identity, stock verification, dispatch, collection
or object production origin. Acquisition and evidence evaluation remain caller work.

The service owns get, assign/replace, clear and change-applicability use cases. Protected
[REST contracts](swagger.yaml) make them callable by external agents. Unsuspended
administrators and trusted internal callers may manage defaults; other users require
active membership in the operator Party partnership plus that exact source grant.
Delegated credentials additionally require `listing-sources:read`/`listing-sources:write`.
The dedicated admin routes enforce administrator/internal authority even for users
with operator membership and a source grant; both route families share the same
assignment, applicability and revision rules.
These operations confer no PartyLocation editing permission. Current user, source,
membership/grant and location authority is validated within one transaction. Lock order
is owner Party, source, user/policy rows, default and selected location; policy/location
locks protect execution from revocation, suspension and retirement races.

The assignment stores only a typed reference and scoped evidence. Source/location
mutations never synchronously hydrate or rewrite dependent ProductListings. Protected
management reads retain active and retired references and evidence for traceability,
without returning PartyLocation content. Authoritative effective-geography reads exclude
retired selections and retain exact asserted geography for active selections.
There is no alternative fallback. Restoration makes the same retained reference eligible;
it never selects a different site. Assignment/replacement and applicability changes require
an active location; clear remains available for a retired assignment.

### Revision, invalidation and derived basis

The default has its own positive CAS revision, initially 1 when never configured.
Every mutation supplies the current expected revision. Changed assignment, evidence,
applicability or clear commits with one revision advance and one immutable minimal
invalidation signal; identical current-revision commands do neither. Stale commands
conflict even for no-ops. Clear retains a revision tombstone so delayed writes cannot
revive an earlier assignment. Revision exhaustion rejects changes while allowing no-ops.
This protected CAS contract follows the documented PartyLocation exception to ordinary
internal storage versions.

`listing_source_location_default_changes` is keyed by source/revision and contains only
source/operator/selected-location IDs, selected location revision, applicability flags,
trusted actor and occurrence time. Clear records a null selection. Historical signals
survive replacement, clearing and source deletion; source configuration is removed with
its source. The selected location cannot be physically deleted while referenced. Private
assignment evidence stays outside CDC/public projections. Signal ownership and later
asynchronous propagation are described in [event flow](events/flow.md#geography-dependency-invalidation).
Effective listing geography now consumes these signals asynchronously; see [its projection contract](#effective-listing-geography-2003).

The authoritative derived read returns each meaning independently with
SOURCE_DEFAULT_DERIVED basis: source ID, current assignment revision, location ID,
current location revision/input revision and meaning. Basis tokens are internal/protected;
public presentation must remove them and evidence. Business association alone leaves
item inventory geography absent. An applicable unresolved site retains its basis but
has no geography/point: pending resolution never invents a country or coordinates.
Partial asserted descriptions remain partial, and mandatory geocoding is unnecessary.

### Listing precedence and suppression (G11 contract)

For each meaning, a listing-specific assertion takes precedence as a whole, even when
partial or awaiting resolution. Never fill its missing fields from a source default or
combine a listing country with a source street/city. Explicit suppression produces
absence and blocks fallback. Clearing an assertion restores inheritance; it differs
from suppression. With inheritance, only an active, disclosed, explicitly applicable
source default supplies derived geography; an absent or ambiguous inventory assertion
leaves item geography absent. Existing default-derived records must retain the revision
basis so G11 can add overrides without changing their meaning. Full listing override
writes and target models belong to G11. Replacing a source default affects current
inherited meaning and later asynchronous projections, never historical listing assertions.

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

The stateless provider capability does not change public REST contracts.

## Durable resolution and Party integration (#2002)

`geo-service::resolution` owns shared resolution orchestration, conservative acceptance,
scope/purpose identity and transaction ports; `geo-postgres` owns private storage and
database coordination. Party services own authorization, PartyLocation identity and
source transitions. The capability supports private Party, User and source namespaces
and explicit shared-business reuse. User-location purpose requires a private User scope;
Party integration defaults to private reuse within the owning Party, including for public
locations. No public cross-user cache is inferred from address equality or a hash.

Committed creation/correction atomically binds eligible address input to durable work.
Caller points take precedence and remove the derived binding; country-only, address-only
and unresolved locations remain valid without provider availability. Retirement detaches
the binding, and restoration explicitly reestablishes eligible work. A changed address or
assertion gets its current scoped input identity. Evidence changes advance
the source marker while unchanged geographic input can reuse permitted evidence. Label
and role edits update the binding guard without paying again. No provider call occurs in
Party creation or correction, a listing write or a search request.

Fingerprints length-prefix exact address, postal, assertion, constraint, hint and language
bytes with scope/owner, purpose, reference release, acceptance policy
and immutable provider interpretation profile. There is no case/whitespace/unit stripping.
The current provider exposes no data-release identifier; its pinned interpretation/terms
profile is retained rather than inventing a supplier release. A changed provider profile
creates a distinct identity. Current input keys omit the removed disclosure context;
the PostgreSQL adapter validates older persisted keys for replay without restoring that
setting. Existing cache scope/owner isolation and paid-attempt accounting are preserved.
Shared inputs never merge PartyLocation IDs or permissions.
Both input and hash remain private personal/business data.

### Acceptance and review

**PRECISION_FIRST_V1** accepts only a unique compatible candidate's country/subdivision
automatically, with its allowed provenance and reference release. It never fabricates a
point or treats provider match scores as confidence. Every returned alternative counts,
including unsupported alternatives. Multiple candidates, contradictions, unverified
constraints and unsupported-only responses abstain as ambiguous; an explicit empty
response is no match. Coarse acceptance retains the complete normalized candidate for
authorized review, separately from its accepted administrative geography.

Protected application use cases inspect permitted candidates, select one compatible
candidate, establish work for a preexisting site, or request retry/re-resolution. They use
the existing Party management/credential policy and owner lock. Candidate acceptance
submits the current location CAS and resolution ID/generation/index; the server checks
current binding/input, provider retention and normalized compatibility. An explicit point
selection requires premises evidence without approximate/unsupported precision. Selection
affects only that location binding, never other locations sharing the result. Repeating
the same selection produces no extra signal. A newer supplied manual correction is made
through the existing correction use case and always wins. No client-supplied verified flag
or new REST/review UI is introduced.

Original address/evidence/caller assertions remain in Party storage. Provider components,
precision, provider/product/data source, region, external reference, attribution and
interpretation profile remain separate permitted evidence; the resolution records its
input marker and policy. Consumers must preserve original country/subdivision assertions
and current lifecycle/retention eligibility. Resolution storage is not a public address
directory or a raw-provider export interface.

### Claims, retries and current eligibility

PostgreSQL uniqueness coalesces an eligible input. A committed claim reserves provider
and scope budget before I/O, with an unexpired lease token and generation fence. Ordinary
concurrent workers coalesce; provider I/O has no open transaction or retained connection.
Completion rechecks the claim, current bindings and provider rights before atomically
storing permitted evidence and its minimal signal. A detached/replaced subject cannot
receive old geography; the same input may still resolve for other eligible bindings.
Uncertain commits and active claims retain failure custody and reconcile on retry.
Binding an eligible cached result also journals a subject-specific resolution signal.
Changing its source revision journals invalidation even when the shared input identity
is unchanged and a reviewed selection is cleared. Replays and presentation-only guard
changes do not emit another geographic signal.

The current runtime makes one SDK attempt per reservation and allows five durable
attempts per generation, with exponential backoff and a shared circuit after repeated
provider failures. Provider and scope daily call/concurrency caps are database authority.
Authentication, configuration, invalid response, unsupported input and storage violations
block for explicit operator correction/retry. Exhausted transient work is FAILED, never
NO_MATCH. Failure classifications are persisted in SCREAMING_SNAKE_CASE; original provider
causes remain available through service error sources and are excluded from runtime logs.

A crash after a paid call and before commit can cause another paid call after lease
expiry. Reservations consume budget even on crashes; there is no exactly-once charging
claim. Manual re-resolution requires the current generation and cannot bypass daily
budgets. An unchanged completed retry reuses eligible accepted, ambiguous or no-match
evidence. Negative results have bounded reconsideration deadlines. Explicit forced
re-resolution invalidates old evidence and review selections before another attempt.
An invalidated in-flight call keeps its concurrency reservation until its fenced reply
or lease expiry, so manual requeue cannot create another concurrent paid reservation.

Current evidence requires the same current source binding and unexpired provider rights
and retention. Readers suppress stale/expired/revoked evidence immediately, including
reviewed points, and recheck bindings after waiting for resolution coordination;
original country assertions remain useful. Bounded geography maintenance in the existing
hourly backend cleanup Lambda physically purges expired, revoked and orphaned provider
derivatives, clears selections,
commits invalidation and establishes a new generation where retention is permitted.
The journal and wake-ups contain no address, scope owner or provider evidence, so erasure
does not leave copies of prohibited derivatives in event history. Dependent projections
must re-read these eligibility rules and consume invalidation/rebuild work, as required
by the [effective listing geography contract](#effective-listing-geography-2003).

### Activation and recovery

Apply the resolution migration before affected runtimes. Paid resolution defaults off
in both stage configuration and provider controls, and storage permission defaults off.
Deploy the compatible CDC router and retained queue/worker before new Party bindings
produce wake-ups; coordinate the selected DMS source with the existing capture checkpoint.
The retained Amazon profile uses the existing reviewed Frankfurt terms gate above.
Provider permission is an operator decision, not evidence that deployment grants rights.
Only enable storage and paid work after confirming the applicable terms, private-address
purpose, attribution and budget. Withdrawal of rights must set storage permission false;
turning off paid calls alone deliberately preserves still-permitted cached evidence.

Only `geo_resolution_wakeups` is selected for CDC, carrying `gres_` identity/generation
through schema-2 jobs. The versioned SQS worker uses partial failure responses and the
ordinary scoped DLQ. The existing hourly backend cleanup Lambda purges ineligible
evidence and publishes fresh durable wake-ups for runnable pending work, including lost
deliveries and expired leases. Recovery advances the generation to fence old jobs while
preserving attempt counts, pending age, backoff eligibility and budget reservations;
it skips live claims, exhausted budgets and inactive provider profiles. Cleanup makes no
provider calls and still purges evidence with paid work disabled. Fresh wake-ups follow
the normal CDC/SQS path, which must be healthy for resolution to resume. Expiry and
negative reconsideration also create wake-ups; there is no separate geography schedule.
Protected Request use cases establish work for preexisting eligible sites; CDC-only
capture does not backfill them. [The worker runbook](durable-worker-runbook.md#geographic-resolution-recovery)
owns operator recovery and safe monitoring.

## Effective listing geography (#2003)

`product-listing-service::geography` owns this enrichment and its query/read ports.
It selects the one explicit source default independently for associated business and
advertised inventory. Business association never establishes item geography. A missing or
retired default gives absence. An unresolved default without reliable submitted facts
gives absence. Reliable submitted country/subdivision remain useful when resolution has no point.
No PlaceId, Party hydration, generic location graph or per-listing provider call is needed.

Submitted country/subdivision and caller position retain precedence over current accepted
resolution. Resolution requires the binding's current input and location revisions and
permitted, unexpired evidence. Active defaults preserve exact caller positions or eligible
resolved positions. Compact derived storage and documents contain only selected location, SOURCE_DEFAULT_DERIVED basis, pinned ISO
release, administrative codes and eligible point/precision. Addresses, postal text,
localized names, source applicability evidence and provider payloads stay out. Inventory
basis conveys the source's assertion, not individually verified stock or custody.

Dependency markers retain listing canonical version, source assignment/applicability
revision, location identity/input/revision/lifecycle, current resolution
identity/generation/review selection/retention deadline and policy versions. They are
internal equality guards, never ordered external versions. Current authorities are locked
and rechecked in the short page transaction. Applicability evidence and geographic values
rehydrate through their domain constructors; invalid stored state fails the page without
committing a projection or opening the public gate. Batched resolution reads follow the
resolver's provider-control, resolution, binding lock order. G11 must replace inheritance **as a whole**
for an explicit listing assertion, and must block inheritance for suppression; partial
assertions must never be merged with source address fields.

Shared change journals atomically establish coalesced dependency work through database
triggers. Work follows indexed source/location/resolution references and an ascending
(source UUID, listing UUID) cursor, with bounded pages, fenced leases and transactional
checkpoints. Newer input resets cursors and fences the old claim. Every canonical listing
event separately establishes durable listing work, so creation after a cursor passed,
including an ID preceding that cursor, is recovered. Changes update compact derived state;
only actual derived changes append ENRICHMENT_GEOGRAPHY_UPDATED and advance the existing
monotonic projection version/current event. Aggregate version, source facts, updated time
and public domain history stay unchanged. Projector and saved-filter percolator receive
the enrichment; price-watchlist, embedding and translation do not.

### Public eligibility barrier and G7 integration

`GeographicQueryGate` reads PostgreSQL on every request, without caching. Pending shared
dependency/lifecycle invalidation, unacknowledged replacement/removal of existing geography,
missing gate authority or expired derived retention closes the gate. Provider storage-right
changes enqueue profile work atomically. Ordinary canonical listing catch-up and creation
remain durable but do not close the gate or advance its epoch. First geographic additions
cannot expose forbidden stale data and do not require a barrier; lifecycle changes and any
replacement of existing geography retain custody until indexing is visible. Coalescing
never downgrades a pending blocking invalidation. G11 must extend the trigger classification
when explicit listing geography changes become canonical inputs.
Replacing/removing a derived value also advances the epoch within the page transaction;
this fences requests when ordinary catch-up discovers expiry before maintenance signals it.
Completion of a fanout page alone never establishes index freshness: projector custody
is recorded only after the externally versioned document/tombstone write is refresh-visible.
Conflict/replay completion explicitly refreshes the product index before confirming custody,
including an uncertain earlier write or a concurrent equal/higher write.

`with_geographic_eligibility` checks before executing a geographic query and again after
execution against the same invalidation epoch. A closed or changed gate suppresses the
**entire** geographic result, including distance ordering, counts and facets. This is a
conservative global barrier: unrelated geographic matches can temporarily be suppressed
while shared or restrictive changes propagate. Sustained invalidation backlog reduces geographic availability; backlog age
and index custody must be monitored. Ordinary non-geographic queries remain independent.
This prevents retired or retention-expired geography from remaining eligible during reindexing.

G7's country/subdivision/radius/facet/saved-filter entry points must wrap their complete
query in this capability; a hit-level serialization check cannot satisfy the contract.
The supplied public batch reader uses it already and reads at most 100 listing IDs in one
query. G7 owns public filter/response/API shapes and the real geographic-query tests;
this feature introduces no public API parameters or override writes. Rebuild/replay and
activation are described in the [worker runbook](durable-worker-runbook.md#listing-geography-recovery).
