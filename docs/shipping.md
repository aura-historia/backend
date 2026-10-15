# Advertised delivery contract

Shipping owns advertised delivery vocabulary and deterministic assessment. Authorized
quotes, order agreements and operational shipments have separate contracts. Services
supply current profiles, listing configuration and explicitly referenced locations;
the domain performs no I/O, clock lookup, geocoding or currency conversion.
[Geography](geography.md) owns geographic assertions and reference interpretation;
[ProductListing](product-listing.md) owns listing identity and source facts.

## Identity, ownership and selection

A ShippingProfile has a stable [object identity](object-ids.md), immutable ListingSource
ownership, positive semantic revision and active/retired lifecycle. Its optional Party
attribution establishes no seller, carrier, arranger, performer or quote issuer. Equal
names, addresses, Party attribution or content never share identities or defaults across
sources. Changes to content or lifecycle advance the revision once; equal commands are
no-ops. A retired profile accepts equal content as a no-op and requires explicit restoration
before correction. Revision exhaustion rejects changes atomically while preserving no-ops.
Rehydration enforces the same invariants as construction.

Each source explicitly selects zero or one active default profile. Retained references
may become absent or retired; assessment reports that dependency and selects no fallback.
Assignment checks source ownership and active lifecycle, including repeat assignment;
clearing an assignment remains possible after retirement.

Listing configuration has three independent parts:

| Part | Meaning |
| --- | --- |
| Selection | Inherit the source default, select an explicit profile, supply inline terms, or suppress inheritance. |
| Rate inputs | Source-scoped classification membership and known available variants. Partial classification sets cannot prove nonmembership. These facts do not themselves set prices or constitute overrides. |
| Overrides | Independently inherit, replace or clear the complete shipping policy and collection policy. Inherit removes an existing override. |

Inherited selection supports both override components. A replacement supplies its own
coverage, applicability, origin and services as a whole. Clear removes an assertion;
an explicit shipping prohibition asserts `NOT_OFFERED`; suppression prevents inherited
terms. These are distinct intents. A shipping override preserves inherited collection
only when its policy is independent and still applicable; collection coupled to the old
shipping policy becomes unresolved until explicitly replaced. Independent overrides can
be evaluated even when the base profile is absent or retired.

EffectiveDeliveryTerms is a derived snapshot of that selection, with the selected profile
identity/revision and dependency reasons. ShippingAssessment is derived from the snapshot
and supplied inputs. A service must resolve fresh authoritative dependencies when it needs
current assessment. The domain does not depend on ProductListing aggregates or implement
cross-context orchestration.

## Authorized profile management and persistence

Protected management uses `listing-sources:read/write` for delegated credentials.
An unsuspended administrator, or current member of the operator Party's active
Partnership with the exact source grant, may operate. Trusted internal principals
use the same service boundary. Party → source → user/grant → shipping state/location
locks hold current authority and compatible location ownership through commit.
The existing narrow source/location access contract supplies these decisions.
Explicit dispatch/collection references assert shipping applicability; they confer
no PartyLocation editing authority or verified fulfillment meaning. Confidential
origins remain asserted protected terms and never create public PartyLocations.
Management detail and evidence never automatically publish collection addresses.

[OpenAPI](swagger.yaml) owns the source-scoped profile list/detail, create, complete
content/evidence replacement, retirement/restoration and default assign/clear routes.
Profile `managementRevision` protects every command, including no-ops, independently
of `termsRevision`. Content/lifecycle changes advance both; evidence-only correction
advances management state without changing normalized terms. Default revision 1
means never assigned; changes retain advancing rows even after clear. Assignment
and repeated assignment require current active, same-source profiles and eligible
referenced locations. Retirement and clear remain possible when a location retires.
Equal valid commands produce no state/history/signal changes. Signed PostgreSQL
revision exhaustion rejects changes atomically while allowing valid no-ops.
Retired profiles require restoration before content or evidence correction.

Creation identity is `(ListingSource, caller, creationKey)`, with one user identity
across Cognito and delegated credentials. Keys contain 1–128 visible ASCII bytes.
Retries compare original validated content and evidence, even after later edits;
matching retries return current state without effects and conflicting retries fail.
Every retry revalidates current source authority. Receipt identity survives source
removal, so deletion cannot make an executed creation key reusable.

PostgreSQL stores native UUID keys and adapter-owned `SHIP_TERMS_V1` terms. Every
schema field, including nullable assertions, is explicit; unknown fields/versions,
invalid codes, bounds, references and constructor failures reject persisted state.
The versioned JSON protocol is shared by transport and persistence; each consuming
adapter supplies its own strict object-reference codec. Public TypeID and stored UUID
representations remain distinct, and core/service code does not depend on JSON codecs.
Source-owned class/variant codes are interpreted only within the profile's immutable
source. Lists read at most 100 summaries using UUID keysets; detail loads one bounded
terms value. Readers never hydrate dependent listings or return evidence. Both API
results and errors use `no-store`.

Normalized current terms, protected revision history and provenance have separate
storage. Evidence contains at most 32 validated assertion/reference pairs, without
buyer addresses, provider payloads or credentials. References are inert links:
writes never fetch them and they never establish caller authority. Complete
replacement corrects the current evidence set; prior evidence remains in protected
immutable history. Profile retirement/source deletion retain profile history,
creation receipts, signals and revision fences without TTL for traceability and
later recovery. Authorized erasure must explicitly remove protected site/evidence
content under an operational maintenance procedure while preserving address-free
identity/revision fences; ordinary profile commands cannot rewrite historical facts.

A source deletion clears and advances its shipping default in the deletion transaction
and retains a `SOURCE_DELETED` fence, including a source never assigned a default.
An exhausted default fence rejects deletion atomically. Profiles retain original source identity and
lose their live source reference; they are unavailable/ineligible. A default retained
after profile retirement continues to identify that retired profile. Missing or retired
explicit references never select another profile. Neither retirement nor source
removal changes a merchant's advertised no-shipping assertion.

## Listing configuration persistence and integration scope

[ProductListing](product-listing.md#listing-delivery-assertions) owns canonical item writes,
optimistic revisions and semantic events. Configuration is stored with its listing in
`LISTING_DELIVERY_V1`; source/profile references use native UUIDs in storage and TypeIDs
in transport. Every field, including nullable selection and rate inputs, is required;
invalid references, unknown fields/versions and constructor failures fail closed.
Whole shipping and collection components use the existing strict terms protocol.

Source-authored profiles and provider profiles have distinct integration scopes. Provider
scope is the exact enabled method/domain for the source. Shopify uses its configured
shop domain; WooCommerce requires an explicitly configured shipping integration domain,
never the source presentation URL. Provider acquisition/configuration owns that binding.
Profile selection must match that scope as well as immutable source ownership. Source management creates source-authored
profiles; provider acquisition must explicitly bind provider profiles before selecting them.
Opaque class/variant identities inherit this scope; edits cannot change the scope while
retaining old facts. A provider assignment without a reliable selection keeps inheritance
blocked. Clearing it never silently assigns a source default. Provider synchronization and
binding acquisition remain owned by the evidence/synchronization contract.

Listing writes validate current active profiles and compatible explicit PartyLocations,
including equal commands using absent default configuration, under the existing source/grant locks. Whole-configuration
clear remains available after dependencies become invalid; provider clear retains the
old integration with no selection, inputs or overrides. Replaced/cleared location
blocks do not borrow or validate discarded inherited sites. Reverse profile/source indexes
support bounded invalidation; shared profile management performs no listing rewrites.
Only protected reads expose item configuration. Public history and ordinary downstream
jobs contain a delivery-change notice or identities, without site assertions.

## Immutable management change signals

Profile state/history/evidence and one minimal `shipping_changes` signal commit
atomically for each changed management revision. Default assignment, clear and source
delete similarly commit with a signal. Signal schema version 1 contains native source
and subject UUIDs, subject kind (`PROFILE` or `SOURCE_DEFAULT`), management/default
revision, profile terms revision when applicable, change kind, actor and recorded time.
Kinds are `CREATED`, `REPLACED`, `RETIRED`, `RESTORED`, `EVIDENCE_CORRECTED`,
`DEFAULT_ASSIGNED`, `DEFAULT_CLEARED`, and `SOURCE_DELETED`. The immutable identity is
`(subject_kind, subject_id, revision)`; default subject identity is its source.
Signals contain no terms, evidence, addresses or dependent listing IDs.

Profile/source and location/profile indexes retain reverse references for later bounded
fan-out. Management performs no dependent listing rewrite or provider/projection call.
These signals establish durable invalidation input; [event flow](events/flow.md) owns
activation and downstream routing. Later consumers reread current authoritative
state, preserve source deletion fences and tolerate duplicate/out-of-order signals.

## Evidence and connector synchronization

`SHIP_SYNC_V1` is the independent ingestion boundary for the external source agent,
product crawler and connectors. [OpenAPI](swagger.yaml) owns callable routes;
[JSON Schema and executable fixtures](contracts/shipping-sync-v1/schema.json) pin the
cross-repository protocol. Producers pin its schema and fixture revision alongside their
generated client. Typed profile and listing commands remain directly usable. Historical
product raw/event versions retain their original interpretation; optional shipping
candidates use this separate stream and cannot reject otherwise valid product facts.

An authenticated manager explicitly binds `SOURCE` or an enabled provider's exact
configured method/domain. The backend issues the epoch; a supplied UUID establishes no
authority. Rotation/replacement and disconnect revoke the preceding epoch. Provider
configuration replacement/removal also fences acquisition. Installation status resolves
an uncertain rotation response. Every capture, status and application revalidates current
source/grant authority. Delegated access requires both listing-source and product-listing
capabilities for the operation. Source deletion leaves identity/receipt fences.

### Fetch custody and completion

1. Read the scope generation/sequence and begin with a stable fetch key **before acquiring
   pages**. Begin retries compare the original request. `providerVersion` records the
   Shopify API or WooCommerce plugin version, or the website adapter version.
2. Upload zero-based chunks in any arrival order. At most 16 chunks, 16 records/chunk,
   256 records/generation and 65,536 input/canonical bytes/chunk are admitted. At most
   16 unexpired open/pending sessions exist per installation. Each session expires after
   24 hours. Object identities and optional provenance associations are bounded and inert.
3. After successful acquisition of every advertised page/capability, finalize with the
   exact chunk/record counts and SHA-256 of the concatenated **binary** ordered chunk
   digests returned by capture/status. Chunk digests cover the backend's canonical
   allowlisted interpretation: recursively sorted object keys, retained array order and
   UTF-8 JSON without whitespace. Native producer observations need no local shipping
   interpretation to build the manifest. Even an empty snapshot uploads one empty chunk.

`components` explicitly selects completeness for profiles, source default and item
membership/inputs, in that order. A snapshot retracts unseen assertions only for selected
components already owned in that scope. Scope ownership cannot migrate implicitly.
Partition large membership sets into stable scopes of at most 256 retained object keys;
use a new reviewed scope/epoch when a partition is exhausted. A product webhook uses a
scoped `DELTA`, never a configuration snapshot or global clear. Duplicate chunks compare
immutable canonical values; changed retry reuse conflicts, including after sealing.
Missing indices/counts/digest or failed capability acquisition cannot activate retirement.
The first finalization fixes the manifest even when chunks are still missing; later
chunks may complete that intent, but a retry cannot shrink it to match partial custody.

Finalization first commits raw custody/completion intent, then atomically commits canonical
application, assertion ownership, normal profile/listing change signals, scope generation
and application receipt. An immutable `shipping_sync_changes` signal contains only native
source, epoch and session identities, generation, version and recorded time. It carries no
scope text, terms or evidence; downstream routing is not activated by this intake.
Profile/default changes do not manufacture listing events.
Status distinguishes `custody`, `rawCapture`, `application`, `applied` and `projection`.
Only `APPLIED` means confirmed canonical completion. `WAITING_DEPENDENCY` returns 424;
missing profiles/listings and withdrawn update targets remain pending without partial
canonical effects. A deletion of an already absent/withdrawn target records a tombstone
without restoring it; independent catalog restoration cannot revive its acquired assertions.
Captured intent can be retried after repair, including after an uncertain commit.
`projection: UNKNOWN` establishes no visibility guarantee. Current canonical shipping is
available through existing listing responses immediately after application.

Delta begin advances the installation's acquisition sequence. Confirmed changes to shared
terms, lifecycle or default assignment also advance the configuration fence, invalidating
membership dependencies in other scopes without rewriting every listing. Application checks
the acquisition sequence and its scope generation
captured at begin. A concurrent delta or competing finalized fetch
fences the old snapshot; its receipt becomes `RECONCILE_REQUIRED` and the scope is durably
dirty. The bounded pending directory also exposes current completed scopes whose
dependencies became dirty; receipts retain historical `APPLIED` alongside current
`reconciliationRequired`. Provider membership cannot support a definite selection/rate
input until a newer full fetch validates those dependencies. Start that fetch from current
fences before declaring completeness. This
is an acquisition fence, not a claim that paginated provider reads were transactional or
that receive time establishes provider chronology. Observation acquisition must fall
between begin and finalization; producers must forward concurrent upstream changes into
the affected scope. Live webhook payloads and cached bulk exports are not current reads:
use them to request a fresh scoped acquisition after begin, or retain their original age
and reconcile. A provider permission/extraction failure preserves old assertions and
their original observation age.

### Interpretation, ownership and age

Backend adapters interpret bounded Shopify profile/variant membership and WooCommerce
ordered country-zone/method/class-rate observations. Native v1 candidates cover known
fixed advertised amounts in currency minor units and explicit charge bases. Shopify mixed
variant profiles retain source associations and unresolved selection; the first priced
variant never establishes coverage. Shopify membership keys use the numeric catalog
source listing ID; the adapter maps a product GID to that ID and retains the native GID
as provenance. WooCommerce classes remain applicability/rate inputs.
Unsupported formulas, postal zones, tax/quantity assumptions or incomplete extraction must
use `UNAVAILABLE` and await a newer supported acquisition; producers must not simplify
them into definite rates. No credentials, wholesale settings, executable expressions,
customer/order/address data or arbitrary provider fields are admitted. These observations
are advertised terms, never carrier quotes. Broader provider interpretation belongs to
the provider adapter work, not an engine in the TypeScript/PHP producer.

External profile mappings preserve Aura identity across refreshes within an epoch.
Profile/default ownership uses committed revisions. Item ownership independently compares
selection, rate inputs, shipping override and collection override against the last applied
values. Explicit manager changes release ownership of the changed components and later
evidence preserves them. Reliable deletion retracts only still-owned assertions; it never
deletes/restores a listing or changes its title, price, images or lifecycle.

Acquisition observation, freshness and expiry are explicit and independent of receive,
management and replay timestamps. Semantic no-ops and evidence-only corrections retain
the prior window. Source defaults and item components retain independent expiry/epoch gates, so disconnect or
expiry cannot expose old acquired assertions or erase a separately managed override.
Only a snapshot explicitly complete for membership can revalidate existing item
dependencies in its scope; a configuration-only refresh cannot clear their fence.
Anonymous cache lifetimes respect these component/default deadlines even for implicit
inheritance and mixed ownership.
Raw custody and pending dependency state are bounded; [worker recovery](durable-worker-runbook.md#shipping-evidence-recovery)
owns repair and retained-content erasure. No shipping worker, provider call, public buyer
assessment endpoint or competing projection writer is introduced by this protocol.

## Shipping on listing reads

[OpenAPI](swagger.yaml) owns the listing response contract. Detail by ID/slug, search,
similar listings, watchlists, saved-search matches and auction catalogues return effective
public advertised terms in `item.shipping`. Anonymous callers and users without a selected browsing
destination receive generic terms. Authenticated users with a permitted selected
browsing destination also receive its evaluation in `userState.shipping`; merely owning
an address selects none. The existing preference can select an owned address or explicit
country/postal assertions. Delegated destination reads require `users:read`; credentials
without it receive generic terms. Hidden and
withdrawn entries disclose no shipping; obsolete discovery hits contribute no current
shipping assertion without failing the rest of the page.

Listing services orchestrate enrichment for the whole page, preserving membership and
order. Shipping resolves the destination once and reads listing/source revisions,
defaults, selected profiles and explicit location dependencies from one repeatable,
read-only PostgreSQL snapshot, bounded to the existing 100-item page maximum. Shared
profiles and locations are read in batches. OpenSearch and listing-geography projections
do not determine current eligibility. No provider, geocoder, carrier or live-rate call
occurs, and no buyer assessment or destination snapshot is persisted.

Private destination reads retain ownership, current user status, delegated permission
and evidence-expiry gates. Submitted assertions take precedence; derived facts fill gaps
only when shared reference-release, country, subdivision and postal assertions agree.
Responses include only effective area facts and required derived-evidence attribution.
Browsing establishes no purchase quantities, selected variants, shipment/order counts or
qualifying values. Missing inputs produce explained uncertainty without inventing prices.
Clients use selected zone/service indices to relate evaluations to `item.shipping.terms`.

Each service option keeps its applicability, eligibility, advertised price knowledge,
charge basis/components and known timing independently. Missing quantities or other
necessary facts produce reasons without inventing charges or cheapest-price claims.
The response retains selected rule order, sanitized effective terms, dependency
revisions and evaluation time. A fixed advertised amount is neither a binding
quote nor a complete landed cost. No provider, geocoder, carrier or live-rate request occurs.

Assertion verification time is independent of management/update time. Source-authored
continuing assertions have no implicit deadline; `FRESH` means they have not crossed an
explicit freshness deadline, not that they were recently recrawled. An acquisition owner
may retain explicit freshness and expiry deadlines. `STALE` preserves the old verification
time and remains usable until expiry; `EXPIRED` cannot support a current definite assertion.
Expired profile evidence removes inherited components while still permitting independently
current item overrides. Expired item configuration cannot authorize selection, rate facts
or overrides. Retired profiles are withdrawn dependencies, with no fallback. Failed recrawls,
evidence-only corrections, no-ops, lifecycle changes and unrelated edits neither erase a
reliable assertion nor renew its verification time. Changing normalized terms verifies
the replacement; a listing configuration change verifies its new whole configuration.
Provider acquisition must preserve its own observation time and reviewed deadlines.

Authenticated responses and all errors use `no-store`. Generic anonymous listing
responses follow discovery caching, capped at the next shipping evidence freshness/expiry
deadline. Destination values, private
bookmark identities, address text, postal codes, points, hashes and raw evidence never
enter diagnostics or public storage. Dispatch and collection sites are omitted from
response terms: a PartyLocation reference establishes applicability, not permission to
publish its management address. Exact public pickup disclosure needs an explicit contract.

## Independent assertions

Eligibility is absent or `OFFERED`, `NOT_OFFERED`, or `SUBJECT_TO_CONFIRMATION`. Pricing is
independently absent, free, fixed, from, an estimated range, externally calculated or on
request. Price on request does not imply confirmation of eligibility, and a price assertion
does not establish that shipping is offered.

Rates retain currency, optional charge basis, optional qualifying scope and known included
or excluded components. Free, externally calculated and requested pricing can retain
an explicit currency or leave it absent; absence never selects a currency for a charge.
Money is in the currency's minor units. Numeric ranges require
ordered bounds in one currency. Item, lot, shipment and seller-order charge bases use their
own positive counts; assessment never substitutes item quantity for another count. Unknown
basis yields no calculated charge. Scaling uses checked arithmetic and preserves fixed,
from and estimated-range semantics. Advertised charges retain exclusions and uncertainty;
they are never checkout totals or authorized quotes. Thresholds use only the explicitly
scoped qualifying amount, with an exact currency match; no FX or unrelated subtotal applies.

Handling, transit and total lead times remain separate duration assertions, with their
own units and optional calendar/business basis and trigger. Collection arrangements and
collection windows remain independent. Assessment does not add ranges with different
bases or invent dates. Service features, arranger and performer remain independent of
pricing components and Party attribution.

Dispatch origins and collection sites are separate explicit assertions. Each references
an active PartyLocation or supplies one whole partial geographic/position assertion.
Business/warehouse roles and advertised inventory do not establish either use. An inline
street-only assertion never borrows a country from another site. Object provenance and
object dimensions establish neither dispatch origin nor packed dimensions. A country rule
needs no origin country; an explicitly referenced dispatch site must still be current
and eligible. Domestic comparison requires a usable origin country from the explicitly
selected origin. Referenced collection sites must remain active for offered collection.

## Coverage and conservative assessment

Coverage retains its immutable geographic reference release, applicability, explicit
exclusions, completeness and declared zone order. Country and subdivision rules are
release-pinned. Subdivisions use exact equality: no parent relationship, successor country,
continent or customs group is inferred. Uninterpreted advertised areas remain unresolved.

Predicates return match, no match or undetermined, with specific reasons. Applicability is
a bounded conjunction of class, variant, quantity range and scoped monetary threshold facts.
A known false fact makes the conjunction false; missing facts are undetermined. With no
selected variant, all known variants must agree; mixed applicability requires selection.
Selected variants must belong to the listing source and, when available variants are
known, to that set. Invalid selections and facts from a different source remain
undetermined even for rules without variant restrictions.

Exclusions precede zone selection. An unresolved exclusion blocks a positive result. The
first matching zone selects its service alternatives; an earlier undetermined zone blocks
later selection. Declared order governs selection, with no implicit specificity sorting.
Every service inside the chosen zone is assessed independently. One applicable offered
service establishes offered shipping; otherwise confirmation, unresolved applicability
and service-specific negatives retain their meanings. Service-specific prohibitions do
not establish a blanket prohibition under partial coverage. Missing matches establish a
negative only under complete applicable coverage. A complete policy outside its applicability asserts
no eligibility. Global prohibition is negative without destination input. Missing inputs,
unsupported postal shapes and absent/retired dependencies never become positive matches.

### Postal interpretation

`POSTAL_RULE_V1` supports country-scoped exact PostalCode comparison using geography's
conservative comparison rules. Its reviewed German prefix/range forms require country
`DE`: prefixes contain one to four ASCII digits; inclusive range bounds contain five
ASCII digits in lexical order. Matching requires five ASCII digits and preserves leading
zeros. Malformed observed shapes remain undetermined. These forms describe advertised
sets, without asserting postcode existence or deliverability. Other country prefix/range
forms require a new explicit review; arbitrary regex, evaluation and global numeric
postcode conversion are unsupported. Unknown language versions fail decoding.

Adapters decode canonical enum codes and references explicitly, call domain constructors,
and reject invalid persisted state. Preserve unknown provider vocabulary at its adapter
boundary rather than coercing it into permission. Buyer destination inputs remain transient
assessment inputs and must not enter logs or shared fixtures.

## Executable examples and shared fixtures

The [domain tests](../src/shipping-core/src/tests.rs) exercise the contract, including the
shared gallery profile: painting inherits EUR 80.00 per item (8000 minor units) and appointment collection;
cabinet inherits the profile with a shipping prohibition. A profile change to EUR 95.00
changes painting shipping while cabinet shipping and independently applicable collection
retain their meanings.

The `shipping-core/test-data` [fixture builders](../src/shipping-core/src/fixtures.rs) and
[synthetic gallery JSON](../src/shipping-core/fixtures/gallery.json) are reusable by
normalization, provider mapping, projection and API tests. They use fixed canonical object
IDs, explicit reference releases and minor-unit money, with no buyer data. The JSON is
illustrative fixture vocabulary; adapters own their persistence and transport formats.
