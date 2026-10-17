# Geography contract

This guide owns geographic meaning, evidence, provider rights and privacy. Exact API
shapes belong in [OpenAPI](swagger.yaml); schema and codecs belong in migrations and
adapters. [Event flow](events/flow.md#geography-dependency-invalidation) owns propagation;
the [worker runbook](durable-worker-runbook.md#geographic-resolution-recovery) owns activation,
rebuild and recovery. [Shipping](shipping.md) owns destination eligibility and delivery assessment.

## Assertions and evidence

- Geographic descriptions are partial values: optional verbatim address text, ISO country,
  subdivision and postal assertions. Missing facts remain absent; present blank, oversized,
  incompatible or invalid values fail through constructors. Subdivision-only evidence
  retains its country association without inventing a submitted country assertion.
- Submitted assertions and provider-derived evidence remain distinct. Provider vocabulary,
  unsupported components and processing state never become invented geographic truth.
  Preserve original Unicode, punctuation, spacing and leading zeros. Comparison keys do
  not rewrite observations or establish postal deliverability.
- Postal comparison is conservative: ASCII case and a conventional separator space may
  be folded for recognized GB/CA shapes; other inputs retain exact comparison. The rules
  follow [UK postcode guidance](https://assets.publishing.service.gov.uk/government/uploads/system/uploads/attachment_data/file/257329/additional-validation.pdf)
  and [Canada Post's PCCF reference](https://www.canadapost-postescanada.ca/cpc/assets/cpc/uploads/files/marketing/2017-postal-code-conversion-file-reference-guide-en.pdf).
- Positions are validated WGS 84 latitude/longitude in degrees with explicit precision.
  Nonfinite/out-of-range values fail; `(0,0)` is valid. Accuracy, when supplied, is
  nonnegative metres; decimal places never imply accuracy. A locality centroid is not
  premises evidence, and a premises point does not establish inventory custody.
- Address equality, shared coordinates or a future Place match never merge PartyLocation,
  UserAddress or Party identities. Business association, advertised inventory, dispatch,
  collection and object provenance are separate assertions.

## Reference release and interpretation

Validation uses the immutable Debian **iso-codes 4.20.1** snapshot, upstream tag `v4.20.1`,
commit `74623235ae99f6e835e1e465017ce4c544ae6b53` from the
[upstream repository](https://salsa.debian.org/iso-codes-team/iso-codes/-/tree/v4.20.1).
The [vendored sources](../src/geo/reference/iso-codes-4.20.1) retain attribution to
2016 Dr. Tobias Quathamer and the LGPL-2.1-or-later license. Redistribute the notices,
license and corresponding data source with generated tables; retain the license's
source and modification obligations.

Country parsing accepts exact uppercase ISO alpha-2 codes; subdivisions require complete
codes in the pinned release. No aliases, private/provider codes or customs zones are
inferred. Persist the release with codes; unknown releases fail. Withdrawn-country lookup
preserves historical interpretation without making withdrawn codes current destinations
or guessing successors. Existing releases remain immutable and supported after updates.

`python3 src/geo/scripts/generate_reference.py --check` verifies pinned source hashes and
deterministic tables offline. A new release requires a separate snapshot, provenance,
hashes and explicit decoding; review code reassignment and retain prior interpretations.
Do not replace a snapshot or blindly update the reused country enum.

Continent is the derived **Aura country grouping v1**, not an ISO field or a writable
address fact. It abstains for transcontinental/dispersed cases where one grouping would
mislead, and groups territories independently. It does not locate a particular address
on a physical continent. Shipping and customs groups have separate policy vocabularies.

## Party locations (#1999)

A PartyLocation is an independently identified association between one Party and a site.
A Party can own several sites; roles describe business use and establish no inventory,
dispatch or collection claims. Partial and unresolved locations are valid. Acquisition
of addresses, website fetching and agentic extraction are outside this feature.

Same-site correction retains identity. Relocation creates a new site and retires the old
one atomically. Retirement excludes current public/inherited geography; restoration is
explicit. Neither operation chooses another source default. Retained locations and grants
block Party deletion. Historical facts are not rewritten by current location changes.

### Authorization and public geography

Trusted internal callers and current unsuspended administrators may manage locations.
Other unsuspended users need an explicit Party-location grant or membership in the Party's
active partnership. Grants and partnership authority are independent; revoking one does
not revoke the other. Only administrator/internal callers manage explicit grants.
Delegated callers also need the corresponding `parties:read` or `parties:write` scope.
Authority and invariant-critical policy are checked in the operation transaction under
the owner Party lock, with current-user authority held through completion.

The administrator location-manager directory reports explicit grants and active Partnership
membership independently, with one row per current unsuspended user. Access filters select
users while preserving both reasons. Revocation removes only the explicit reason; dissolved
Partnerships and deleted users confer no directory access. Identity is limited to the
administrator-visible email and optional names; grants have no stored timestamp. Exact
filters, stable pagination and error contracts belong in [OpenAPI](swagger.yaml).

Active PartyLocations permit exact public geography; there is no disclosure setting.
Retired sites are ineligible. Exact eligibility does not upgrade coarse geocoding evidence.
Management reads, raw addresses, applicability evidence and revision tokens remain
protected; private UserAddresses and origins never become public locations. Caller
references establish neither verified facts nor caller authority and are never fetched
by ordinary writes. Geographic content and evidence must not enter diagnostic logs.

### Retry, revision and persistence guarantees

Ordinary creation always assigns a new identity, including for equal content. These
interactive management operations are explicit [keyless retry exceptions](command-retries.md).
Relocation still retires the old location and creates its replacement atomically under the
old expected revision. Repeating a committed relocation with that revision conflicts and
creates no second replacement.

Protected mutations require the current CAS revision, including no-ops. Changed state
advances it once; identical current-revision commands emit no write or signal. Geography,
position or evidence changes also advance input revision; label, role and lifecycle
changes do not rewrite geographic inputs. Exhaustion rejects changes but permits no-ops.
This explicit management CAS is a narrow exception to ordinarily hidden storage versions.
Minimal committed revision signals carry no address, point or evidence. Lists use bounded
keyset traversal and reevaluate authority on every page.

## ListingSource location defaults (#2000)

A source selects zero or one explicit operator-owned PartyLocation. There is no Party
fallback, automatic site selection or separate default per role. Associated business and
advertised inventory each need their own scoped evidence on that reference. Legal or shop
addresses alone do not establish inventory, seller identity, pickup or dispatch.

Current unsuspended administrators/internal callers may manage defaults; other users need
active operator-partnership membership and the exact source grant. Delegated credentials
add the corresponding listing-source scope. Admin routes require administrator authority.
Defaults confer no location-editing rights. Authority, source ownership and active location
eligibility are checked together, following Party → source → user/policy → default/location
lock order. Protected reads retain retired references for traceability without returning
location content; public inheritance excludes them.

Assignment, applicability changes and clear require the current independent CAS revision.
No-ops are silent; clear retains a revision tombstone. Assignment and applicability changes
require an active site, while clear remains possible after retirement. Minimal invalidation
signals commit with changes and survive source deletion. Fan-out is asynchronous.

Future listing assertions replace inheritance **as a whole**, even when partial; suppression
blocks fallback and differs from clearing an assertion. Never combine a listing country
with an inherited street. Override writes, marketplace actors, canonical Places and
structured international addresses remain deferred issues.

## Geocoding capability

The service capability accepts validated address text plus explicit optional assertions,
constraints, ranking hint and language. Assertions do not silently become provider filters.
The consuming service owns authorization, purpose, acceptance and reuse scope; the provider
adapter owns protocol, credentials, bounded I/O and mapping. No provider call occurs in an
ordinary location/address write, listing write or search request.

An explicit empty response is no match. Every returned alternative counts toward ambiguity,
including unsupported candidates; a limit-truncated single result is not proof of uniqueness.
Scores describe matching, not probability. Unknown precision, contradictory components,
unverified constraints and technical failures remain distinguishable.

The initial adapter uses [Amazon Location Places v2 Geocode](https://docs.aws.amazon.com/location/latest/APIReference/API_geoplaces_Geocode.html)
with SigV4 and the reviewed Frankfurt profile. AWS longitude/latitude arrays are explicitly
mapped; invalid coordinates fail. Point addresses map to premises; interpolated addresses
to street; estimated/inferred addresses and POIs remain broader evidence. Provider region
abbreviations are not ISO subdivisions. Only the reviewed Canadian `BC` → `CA-BC` mapping
is supported; further mappings require explicit review. External PlaceIds stay provider
references. Interpretation profiles are immutable and changes require deliberate cache
invalidation. Synthetic adapter tests establish protocol behavior, not worldwide accuracy.

### Provider terms and retention gate

The reviewed profile is **Amazon Location Places v2 / default data / eu-central-1 /
October 2026**. Operators must confirm applicable account/source terms, intended reuse,
private-input purpose, attribution and budgets before enabling retained resolution.
The profile records default data rather than claiming a specific underlying supplier.

[AWS Service Terms §82](https://aws.amazon.com/service-terms/#82._Amazon_Location_Service)
and the [intended-use contract](https://docs.aws.amazon.com/location/latest/developerguide/places-intended-use.html)
govern retention. Retained requests explicitly declare `IntendedUse=Storage`; single-use
results cannot be persisted or cached, including their derived components. Storage is the
API value; **Stored** is the [pricing category](https://docs.aws.amazon.com/location/latest/developerguide/places-pricing.html).
Storage permission does not grant unrestricted resale, bulk collection, public geocoding,
map display or waiver of personal-data erasure. Review [current pricing](https://aws.amazon.com/location/pricing/)
when approving paid evaluation or production volume.

The adapter conservatively applies the HERE Japan storage prohibition to all default data:
Japan-constrained retained requests fail before I/O; Japanese, missing or contradictory
result countries fail before retained evidence is released. Other providers/products/regions
need separate review. Supplier processing may occur outside the AWS region; endpoint region
alone is not a data-residency guarantee.

Persistence owners check storage permission before retaining any derivatives and preserve
reference release, provenance, attribution and profile. Private reuse stays isolated to its
owner and purpose; address equality or hashes never authorize cross-user or public reuse.
Revoke storage permission when rights cease; disabling paid calls alone leaves permitted
cached evidence usable. End-user terms/product documentation must conspicuously link the
[AWS attribution page](https://docs.aws.amazon.com/location/latest/developerguide/data-attribution.html)
and retain required supplier notices. Maps require additional review.

Logs exclude queries, private coordinates, provider identifiers/messages/payloads and
credentials. Public diagnostics are redacted; original causes remain available for controlled
diagnosis and must not be dumped into logs. Keep SDK HTTP/body tracing disabled.

## Durable resolution and Party integration (#2002)

PostgreSQL coalesces exact eligible inputs within explicit scope and purpose. Fingerprints
include original bytes, assertions, constraints, hints, language, reference release,
acceptance policy and provider profile. Party reuse defaults to the owning Party; user
reuse is private to the owner. No shared cache follows from matching addresses.

Creation/correction commits binding and durable work together. Caller points take precedence
and detach derived bindings; country-only or unresolved inputs remain useful without a
provider. Retirement detaches work; restoration explicitly reestablishes it. Input changes
fence old replies; presentation-only changes update guards without paying again.

Business policy **PRECISION_FIRST_V1** automatically accepts only administrative geography
from a unique compatible candidate. Protected application operations can inspect, retry or
select a compatible candidate using current location revision and resolution generation.
Explicit point selection requires premises evidence. Selection affects only its binding,
not other locations reusing the result; submitted assertions always retain precedence.

Claims reserve provider/scope budgets before I/O and retain no transaction during the call.
Completion rechecks lease, generation, current bindings and storage rights, then commits
permitted evidence and a minimal signal. Reads suppress stale, expired or revoked evidence
immediately. Database coordination follows provider → resolution → binding lock order.
Read-only repeatable snapshots use the same permission and logical-expiry gates without
acquiring mutation locks; [shipping assessment](shipping.md#shipping-on-listing-reads)
uses these current facts instead of the listing-geography projection.

Retries are bounded, with backoff, concurrency/daily budgets and a circuit for provider
health failures. Unsupported input remains an input failure; errors never become no-match.
A paid-call/commit crash can incur another call after lease expiry. Reservations and stale
reply fences survive invalidation; exactly-once charging is not promised. Expiry, revocation,
orphan cleanup and lost-delivery recovery are bounded and run even with paid calls disabled.
The [worker runbook](durable-worker-runbook.md#geographic-resolution-recovery) owns recovery.

## Effective listing geography (#2003)

The backend retains source business association and advertised-inventory applicability
separately. Public listing `geography` uses eligible advertised inventory: a business-only
association supplies none. Source-derived basis is labelled `SOURCE_DEFAULT_DERIVED`; it
never claims individually verified custody. Submitted geography precedes current permitted
resolution. Missing, retired or wholly unresolved sites supply absence.

Compact projections contain location identity, basis, ISO release/codes and point/precision,
never raw addresses, postal text, applicability evidence or provider payloads. Dependency
markers cover listing, assignment, location, resolution and policy revisions. They are
internal equality guards; monotonic projection versions fence external writes.

Committed dependency changes establish coalesced, paginated work with leased claims and
transactional checkpoints. Newer changes reset cursors and fence old claims. Listing events
also establish work so concurrent creation cannot escape a source traversal. Only changed
derived values emit geography enrichment; canonical revision, updated time and public
history remain unchanged. No per-listing geocoding or per-result Party hydration occurs.

### Eventual projection consistency

Listing search and geographic filter matching use available projections while durable
invalidation catches up. Retirement, restrictive applicability, rights revocation and expiry
can briefly leave earlier geography in results, ordering, counts and continuation. There is
no global query gate. Writers reread current authority; versions/tombstones fence stale
writes. PostgreSQL is authoritative; OpenSearch is a rebuildable, eventually consistent read
projection. Projector acknowledgment means a write was accepted or safely fenced as stale;
search visibility follows the normal OpenSearch refresh cycle. Public Party-location
discovery reads PostgreSQL eligibility independently. See [listing recovery](durable-worker-runbook.md#listing-geography-recovery).

## Geographic discovery (#2004)

Listing searches and saved filters accept one `geography` predicate. Country/subdivision
sets use OR within a set; different predicates use AND. Missing facts never match. Radius
and distance order require an explicit numeric origin; addresses, IP, bookmarks and browsing
preferences do not implicitly resolve one. Text/saved-address origins remain future work.

Radius is positive, bounded to the equivalent of 20000 km, and preserves supplied units.
Strict proximity requires premises precision; approximate mode also admits street/locality/
representative evidence. Arc distance is inclusive and uses the same point for filtering
and ordering, with deterministic listing-ID ties. It is not journey distance. Display rounds
to 0.1 km for premises, 1 km for streets and 10 km for broader evidence. Distance ordering
conflicts with ordinary sort and uses lexical text matching. Continuation retains existing
FX/sort state and requires the same query.

Public Party-location discovery returns each matching active location with its Party and
asserted/derived basis, using bounded keyset pages and batched resolution reads. Several
sites of one Party remain separate results. Public views omit management evidence and
revisions. Geographic HTTP responses are `private, no-store`, including anonymous searches.

Saved geographic predicates contain private, release-pinned coordinate snapshots. Replacing
the predicate replaces its origin; unrelated patches or address/preference edits never move
it. Owner authorization and account erasure apply. [Periodic matching](events/flow.md#periodic-saved-filter-matching)
evaluates saved geographic predicates through ProductListing search, subject to its
projection lag; geography events do not directly match saved filters.
[OpenAPI](swagger.yaml) owns field shapes and errors.

## Private user address books and browsing destinations (#2005)

A UserAddress is a named editable bookmark with a server-generated identity. Each ordinary
create assigns a new ID; equal content can represent distinct bookmarks. Creation follows
the [keyless retry exceptions](command-retries.md). Deletion retains an ID tombstone until
account erasure, independently of creation. Replacements/deletion require current CAS;
label changes do not repeat resolution. Protected views expose status, eligible derived
geography and attribution, with batched resolution for lists.

Each user has at most one browsing destination: none, an owned address, or explicit
country/postal assertions. Selection/clear is atomic under preference CAS; deleting the
selected entry clears it without choosing another. An explicit request choice, including
none, takes precedence over preference. Neither preference nor language/currency/IP proves
residence, billing, nationality, tax domicile or deliverability.

Private policy **AUTOMATIC_USER_BROWSING_V1** accepts a point only from exactly one usable
compatible premises candidate, with a requested candidate limit of at least two. Submitted
country/subdivision/postal assertions must agree; unsupported alternatives count toward
ambiguity. A unique coarse result can supply administrative facts only. Users correct
uncertain results through ordinary replacement; no candidate-selection/retry API exists.
Provider failures retry under bounded policy. Frontend autocomplete is optional and has
independent licensing; selected text/provider IDs never establish verified backend facts.

Only current unsuspended owners may access addresses/preferences; delegated callers require
the corresponding user scope. Success and errors are `no-store`. Content remains outside
public indexes, logs and ordinary CDC/jobs; only compact resolution wake-ups leave storage.

### Deletion, erasure and retained snapshots

Deletion erases label/submitted geography and detaches resolution. Replacement also detaches
the prior input; deletion of the last private binding erases provider input/evidence and
pending wake-ups while preserving a content-free worker fence and outstanding reservation.
An in-flight request cannot be recalled, but its reply cannot restore erased content.
Account deletion cascades through addresses, preferences and saved filters; database hooks
ensure erasure also runs for direct deletion. No old address revisions remain as history.

Address edits/deletion do not move or erase independently retained saved-filter snapshots;
users manage those separately and account erasure removes them. Future orders must likewise
snapshot accepted destinations. Active-database erasure does not instantly erase retained
backups or provider records: operators manage snapshot expiry, and restores must replay
erasures before serving data or enabling resolution. Provider retention follows its terms.
