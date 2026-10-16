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
`LISTING_DELIVERY_V2`; source/profile references use native UUIDs in storage and TypeIDs
in transport. Every field, including nullable selection and rate inputs, is required;
invalid references, unknown fields/versions and constructor failures fail closed.
Whole shipping and collection components use the existing strict terms protocol.

Source-authored profiles and provider profiles have distinct integration scopes. Provider
scope is the enabled native method (Shopify or WooCommerce) within the authenticated listing
source. Producers acquire and interpret provider data externally, then push typed profiles,
rates, classifications and memberships through Aura APIs. Backend shipping paths neither
accept provider domains nor perform provider HTTP requests. Provider addresses are not
shipping identity. The V2 persistence migration removes obsolete V1 domain metadata.
Profile selection must match the source/provider scope. Opaque class/variant identities
inherit that scope; edits cannot change the provider method while retaining old facts.
A provider assignment without a reliable selection keeps inheritance blocked. Clearing it
never silently assigns a source default.

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
Provider acquisition preserves its own observation time and reviewed deadlines through
the incremental import contract below.

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

## Incremental typed imports

External agents acquire site policies only for sources without enabled native Shopify or
WooCommerce ingestion. Apps/plugins acquire and map their provider data into the same
canonical terms; the crawler may call these use cases directly. No producer performs
buyer assessment. Unsupported/omitted facts and failed or incomplete reads produce no
writes. Only a successful complete provider-scope read may justify explicit removals.
There is no whole-source activation: create/update profiles before assigning them, and
retain/retry each failed record independently.

[OpenAPI](swagger.yaml) owns the optional import field on existing profile/default and
listing delivery commands. Producers create and replace profiles through ordinary POST/PUT.
Import integration scope must match an enabled native method on the source. SOURCE imports
are restricted to non-native sources. Acquisition and provider mapping belong in the Shopify
app, WooCommerce plugin or external non-native producer; the backend validates, stores and
assesses their pushed typed assertions.

objectKey identifies a profile within its immutable source/producer/integration namespace;
requestKey identifies one exact operation in that namespace. Creation also retains the
existing original-request creationKey. Recover identities through the existing profile
list's objectKey filter; provider lookup also supplies integrationMethod.
The normal summary array contains zero or one profile, including retired/manual profiles.
Exact committed retries return current committed state without repeating effects for
90 days; changed request-key reuse conflicts permanently. Current source/grant/integration
authority is checked before replay, including the outer asynchronous command receipt.
Address-free hashed identity/request fences survive source deletion and replay expiry.
No raw provider payload, capture receipt or reconciliation session is retained.

Ownership follows the existing coherent components: whole profile/default, listing
selection, rate inputs, whole shipping override and whole collection override. An import
can claim an absent assertion and maintain/remove its own assertions. Explicit manual
assertions protect their component even when equal to an imported value;
this ownership handoff has an independent validity signal and does not renew evidence or
fabricate a terms/listing change. Explicit clears and equal assertions retain protection
even before the first import. Implicit source-default inheritance accompanying another
component's edit does not claim selection ownership. Changed components, including clear/removal tombstones,
remain protected through current source locks
and optimistic management/default/listing revisions. Other components remain independent.
Reconnection permits newly acquired work by the same producer to reconcile its previous
assertions through a fresh observation; an ordinary no-op cannot restore their eligibility.
It cannot reclaim a manual component. After integration replacement, the same
producer explicitly clears its old scoped listing assertions before assigning new scoped
profiles/facts. Profile object identity stays integration-scoped; old class/variant facts
never move implicitly. Provider membership without a reliable
selection stays unresolved and never falls back to a source default.

Every imported assertion requires an explicit successful observation when created or
semantically replaced, including a first claim of an equal implicit selection.
Explicit removal of owned components does not require or renew an observation.
acquiredAt is acquisition time; freshUntil and expiresAt are
reviewed deadlines, ordered after acquisition, with expiry at most 30 days later.
Acquisitions must follow the latest source/integration authority change and cannot be future dated or already expired.
A new acquisition of unchanged content/evidence explicitly supplies observation to refresh
validity without changing terms/management/listing revisions or emitting a listing semantic
event. Evidence-only corrections, lifecycle commands, retries and ordinary equal writes do
not renew validity. A correction that changes only evidence cannot carry an observation.
Acquisition ordering uses PostgreSQL's microsecond precision; a new request key cannot
turn the same acquisition into a fresh observation.
Increasing producer sequence fences delayed work per touched component and source lifecycle;
receive time establishes no provider order. After a conflict, refetch and reconcile actual
facts and ownership; never attach a new revision or acquisition timestamp to an old payload.

Current snapshot reads exclude disconnected imported profiles/defaults and independently
expire listing components, preserving manual overrides. Imported membership expiry removes
selection authority and blocks fallback. The listing evidence summary uses the oldest
component acquisition/freshness; validityDeadline caps shared cache lifetime at the next
imported component/default transition. Profile windows retain the B04 expiry contract.
Source deletion, operator replacement, ingestion configuration removal/rebinding, source
grant/member revocation and user/partnership authority changes advance an internal source
revision and acquisition boundary. Producers never read or supply that revision.
These authority changes conservatively invalidate imported assertions for the source;
manual components remain eligible. Regranting or reenabling the same provider method does not
revive old imports. Explicit
revocation is immediate in authoritative reads; the 30-day maximum expiry bounds abandoned
producer cleanup. Normal anonymous discovery cache limits still apply pending B09 fan-out.

Persistence retains the source lifecycle boundary, profile identity/ownership, default ownership,
listing component ownership/windows, hashed request fences and immutable
shipping_validity_changes. Each exists respectively to reject old authority, recover stable
identity, protect concurrent manual edits, expire components independently, recognize exact
retries, or invalidate observation-only changes. Validity signals contain source/subject IDs,
independent revision, kind and recorded time; they carry no terms, addresses or evidence.
[Event flow](events/flow.md) owns downstream activation.

| Outcome | Producer action |
| --- | --- |
| Synchronous 200 | Record committed state; search visibility remains eventual. |
| Async admission report | Record queue acceptance only; application still checks current authority/dependencies. |
| 404 SHIPPING_IMPORT_DEPENDENCY_MISSING | Keep the record and retry after its catalog listing exists; no product is created/restored. |
| Missing profile / ineligible reference | Create/restore the owned profile first, then retry assignment with reconciled current state. |
| 409 SHIPPING_IMPORT_IDENTITY_EXISTS | Resolve the producer object key, read its profile and reconcile. |
| Other import/revision 409 | Refetch current state/configuration; reconcile or discard superseded acquisition. |
| 503 / lost response | Retry the identical request and keys; a missing response does not establish rollback. |

[Canonical request/response fixtures](fixtures/shipping-imports.json) are exercised by
the API and queue codec tests. For S01, regenerate the Shopify app's pinned
@hey-api/openapi-ts client from this specification. For W01, update its pinned
OpenAPI Generator input and operation allowlist with profile list/create/get/replace/lifecycle,
default commands, existing provider configuration, and changeProductListingDelivery
(or its existing asynchronous submission). Preserve the producer backfill pattern of
retaining the exact pending request/key until confirmed admission/application; only
successful complete provider reads generate explicit removals. External-agent SOURCE
examples apply only to non-native sources.
