# Advertised delivery contract

Shipping owns advertised terms and deterministic assessment. Services supply current
profiles, listing configuration and explicitly referenced locations; the domain performs
no I/O, clock lookup, geocoding or currency conversion. Authorized quotes, order agreements
and operational shipments require separate contracts. [Geography](geography.md) owns area
assertions and reference interpretation; [ProductListing](product-listing.md) owns item facts.
[OpenAPI](swagger.yaml) owns exact HTTP shapes, codes and bounds.

## Identity, ownership and selection

A ShippingProfile has stable source-owned identity, positive semantic revision and
active/retired lifecycle. Optional Party attribution establishes no seller, carrier or
fulfillment role. Equal names, addresses or content never share identity across sources.
Content/lifecycle changes advance the terms revision once; equal valid commands are no-ops.
Retired profiles require restoration before correction. Revision exhaustion rejects changes
atomically while preserving valid no-ops. Rehydration enforces constructor invariants.

Each source explicitly selects zero or one active, same-source default. Missing or retired
references stay traceable and select no fallback; clear remains possible after retirement.
Listing configuration keeps these independent parts:

| Part | Meaning |
| --- | --- |
| Selection | Inherit the source default, select an explicit profile, supply inline terms, or suppress inheritance. |
| Rate inputs | Source-scoped class membership and known available variants; partial sets cannot prove nonmembership. |
| Overrides | Independently inherit, replace or clear the complete shipping and collection policies. |

Clear removes an assertion, prohibition asserts `NOT_OFFERED`, and suppression blocks
inheritance. Setting `INHERIT` removes an override. A replacement supplies a whole policy,
including its applicability, coverage, sites and services. Independent collection survives
a shipping override if still applicable; collection coupled to the replaced policy becomes
unresolved until replaced. Independent overrides can remain usable without a base profile.
Effective terms and assessments are derived snapshots, never copied profile prices.

## Authorized profile management and persistence

An unsuspended administrator or current member of the operator Party's active Partnership
with the exact source grant may manage profiles. Delegated credentials additionally require
`listing-sources:read/write`; trusted internal callers use the same service boundary.
Current authority and compatible PartyLocation ownership remain locked through commit.
A shipping site reference confers neither location editing authority nor public disclosure.

Profile `managementRevision` protects every command, including no-ops. Content/lifecycle
changes also advance `termsRevision`; evidence correction advances only management state.
Default revision 1 means never assigned; clear retains an advancing revision tombstone.
Assignment, including repeats, requires active same-source profiles and eligible referenced
locations. Retirement and clear remain possible after a location retires. Valid semantic no-ops
preserve management revisions/history; ownership and observation signals remain independent.

Creation identity is `(ListingSource, caller, creationKey)`, with the same user identity
across Cognito and delegated credentials. Exact retries compare original validated content
and evidence, return current state, and recheck current authority. Conflicting reuse fails;
receipts survive source deletion. Lists are bounded summary reads; details omit evidence.
Management responses and errors are `no-store`.

Public shipping management requests/results and pure use-case mappings are shared by REST and
[MCP](mcp.md#shipping-management). The [contract tests](../src/aura-historia-contracts/tests/shipping_management.rs)
check complete terms schemas, required-nullable attribution and adapter-owned public versus
persisted references; `shipping-json` remains the authoritative terms codec.

PostgreSQL is authoritative. Adapter-owned `SHIP_TERMS_V1` and `LISTING_DELIVERY_V1`
protocols require every field, including nullable assertions. Unknown fields/versions,
invalid codes, references and bounds fail closed through domain constructors. Each adapter
supplies its reference codec: public TypeIDs and stored UUIDs remain distinct. Core and
service crates do not depend on those JSON representations.

Current terms, protected immutable revision history and bounded evidence are separate.
Evidence links are inert: writes never fetch them or derive authority from them. Evidence
must contain no buyer addresses, raw provider payloads or credentials. Retirement and
source deletion retain history, receipts and revision fences without TTL. Authorized erasure
needs an explicit maintenance procedure that removes protected site/evidence content while
preserving address-free identity/revision fences; ordinary commands cannot rewrite history.

Source deletion atomically clears/advances its default and records a deletion fence, even
if no default was previously assigned. Revision exhaustion rejects deletion. Profiles
retain their original source identity but become unavailable. Neither retirement nor source
removal invents a merchant no-shipping assertion.

## Listing configuration persistence and integration scope

[Listing delivery writes](product-listing.md#listing-delivery-assertions) use the ordinary
listing transaction, optimistic revision and semantic event. Protected inspection and
revision-checked updates support both administrators and authorized source operators;
delegated credentials require `product-listings:read/write`.

Source-authored and provider profiles have distinct integration scopes. Provider scope is
an enabled Shopify or WooCommerce method within the authenticated source. Producers map
provider data externally and push typed terms; backend shipping accepts no provider domain
and makes no provider HTTP requests. Profile selection and opaque class/variant codes must
match that scope. Integration changes require explicit removal of old scoped assertions.
An unresolved provider assignment blocks inheritance instead of selecting a source default.

Writes validate current profiles and explicitly retained locations, including equal commands.
Whole-configuration clear remains possible when dependencies become invalid: source-authored
configuration returns to default inheritance, while provider configuration retains its method
with no selection, inputs or overrides. Discarded sites are not borrowed or validated.
Only protected reads expose configuration. Public history reports `DELIVERY_CHANGED`
without terms/sites; shared profile edits never rewrite dependent listing aggregates.

## Immutable management change signals

Profile state/history/evidence and a minimal `shipping_changes` signal commit together for
each changed management revision. Defaults and source deletion do likewise. Signal version 1
contains source/subject identity, subject kind, management/default revision, profile terms
revision when applicable, change kind, actor and recorded time. Identity is
`(subject_kind, subject_id, revision)`; default subject identity is the source.
Kinds cover creation, replacement, retirement, restoration, evidence correction, default
assignment/clear and source deletion. Signals contain no terms, evidence or dependent IDs.

Import observations, ownership handoffs and authority changes have separate immutable
validity signals. They invalidate derived state without fabricating semantic listing
changes. [Event flow](events/flow.md#shipping-management-signals) owns routing and codecs;
consumers reread current state and tolerate duplicate/out-of-order signals.

## Shipping on listing reads

Detail by ID/slug, search, similar listings, watchlists, saved-search matches and auction
catalogues include public, destination-independent `item.shipping`. An authenticated user
with a permitted selected browsing destination also receives private `userState.shipping`.
Merely owning an address selects none. Delegated destination reads require `users:read`;
other callers receive only the public object. Hidden/withdrawn or obsolete discovery hits
disclose no shipping.

The response purpose selects the representation, even inside detail-shaped collection items:

| Read | `item.shipping` | `userState.shipping` |
| --- | --- | --- |
| Collection: search, similar, watchlist, saved-search matches, auction catalogue | Summary: eligibility determinable without a destination, required inputs, explanations, collection summary | Summary for the destination, at most one price preview, required attribution |
| Single listing: detail by ID/slug, watchlist update | Summary fields plus effective terms | Minimal destination, attribution, summary, assessed options of the destination's zone, collection summary |

Collection summaries never grow with the advertised policy: no terms, zones, service lists,
per-zone conversions or pointers into omitted terms. Details carry the material terms
(coverage completeness and zone order, exclusions, applicability, rates and components,
timing, features, actors, pickup arrangements and geographic reference releases). Options
are self-contained: each names its service and copies its advertised rate. Neither carries
the `SHIP_TERMS_V1` marker, revisions, profile, evidence or evaluation times.

Public reads omit absent optional values and empty optional collections at every level and
never serialize null. Missing eligibility is unknown or unasserted; only `NOT_OFFERED` is a
negative. A summary or terms object without information is omitted, and missing shipping is
never a no-shipping assertion; missing `userState.shipping` means no disclosed destination
assessment. These read projections leave persisted, import, command and management protocols
unchanged.

Undetermined outcomes are explained by a bounded vocabulary derived from the assessment,
never a raw reason trace. Determined outcomes carry neither field.

| Field | Values |
| --- | --- |
| `requiredInputs` | `DESTINATION`, `DESTINATION_COUNTRY`, `DESTINATION_SUBDIVISION`, `POSTAL_CODE`, `QUANTITY`, `LOTS`, `SHIPMENTS`, `SELLER_ORDERS`, `VARIANT` |
| `explanations[].code` | `INCOMPLETE_COVERAGE`; `QUALIFYING_AMOUNT_REQUIRED` with its `scope`; `UNRESOLVED_TERMS` for seller-side origin/class facts, uninterpretable areas and dependencies; `SHIPPING_UNDETERMINED` for shipping-dependent collection; `NOT_ASSERTED` for applicable terms without eligibility |

Unasserted terms, dependency notices, earlier-zone pointers, negatives and price facts shown
by the rate add no explanation. An option explains only its own applicability; charge reasons
concern its price.

The price preview is the advertised rate of a service establishing the summarized `OFFERED`
or `SUBJECT_TO_CONFIRMATION` eligibility in the destination's selected zone, after exclusions
and zone precedence. Services with undetermined conditions never supply it. Every candidate
needs an explicit basis. Several candidates are compared only with one basis and exact fixed
or known-currency free prices in one currency: the lowest wins, then the first advertised.
Otherwise there is no preview. It keeps its service, basis and price kind, never implies a
purchase quantity, total or quote, and is not a global cheapest-rate ranking.

Services enrich the whole page, preserving membership/order. They resolve the destination
once and batch current listings, sources, defaults, profiles and explicit locations from one
repeatable read-only PostgreSQL snapshot. OpenSearch and listing-geography projections do
not determine current eligibility. No buyer assessment/destination snapshot is persisted.

Private destination reads enforce ownership, current user status, delegated permission and
geographic evidence validity. Submitted assertions take precedence; derived facts fill gaps
only when reference-release, country, subdivision and postal assertions agree. Responses
retain only effective area facts and required evidence attribution. Browsing supplies no
purchase quantities, selected variants or qualifying basket amounts, so it calculates no
charge and missing inputs remain explained uncertainty.

Assertions continue until changed, withdrawn or their source loses authority. Observation
age never removes terms or caps caching. Successful acquisition time is independent of
management/update time; failed reads, retries, lifecycle commands, evidence-only corrections
and ordinary no-ops do not renew it. Semantic replacement records the new assertion.

Dispatch/collection sites are omitted from public terms; a reference does not authorize
publishing its management address. Exact pickup disclosure needs its own contract.
Destination values, address identities/text, points, hashes and raw evidence never enter
logs or public storage. Authenticated responses and errors are `no-store`. Generic anonymous
responses follow discovery TTLs, shortened where geographic provider retention requires it;
retention deadlines and evaluation times stay internal.

## Effective shipping discovery projection

A rebuildable PostgreSQL summary retains reverse dependency identities per listing.
Transactional invalidation coalesces work by subject; workers traverse bounded UUID pages
with generation/lease-fenced checkpoints. New listings and assignment changes establish
independent work even after a shared traversal cursor has passed. Imports become visible
record by record, without a whole-source completion barrier.

Projection uses the same current selection, authority and assessment semantics as reads.
Its public document contains independent collection eligibility, explicit global prohibition,
a conditional/unsupported flag and nested country/service offers. Only definite whole-country
applicability creates an offer. Postal exclusions, unknown origins/variants, missing basket
inputs and earlier unresolved zones cannot become unconditional matches. Exact fixed/free
per-item amounts retain their own currency/minor units; other price knowledge stays conditional.
The bounded offer cap retains a conditional flag on truncation. Missing offers never prove
unavailability. No sites, private evidence, buyer data or full policies enter the public index.

Changed summaries advance [listing freshness](product-listing.md#freshness) and the existing
complete-listing projection fence and emit shipping enrichment; unchanged summaries refresh
dependencies only. A shared profile or default change therefore advances only listings whose
effective summary changes. Complete document replacement
and external version fences preserve newer price/geography state and deletion tombstones.
Profile revisions never become global OpenSearch versions.

Membership, counts and order remain eventually consistent. Returned terms use current
PostgreSQL state without post-filtering indexed pages, so an old-rate hit can already show
new terms. Anonymous caches can retain earlier responses only within their bounded TTL.
Shipping work relies on CDC/SQS, durable continuations, retries and controlled DLQ redrive;
there is no scheduled shipping expiry or pending-work scan. Geography owns evidence-retention
invalidation. [The worker runbook](durable-worker-runbook.md#listing-shipping-recovery) owns
monitoring, bounded rebuilds and recovery of both projection stages.

## Shipping display currency

Detail terms keep advertised source prices. A rate whose amount is in another currency adds
`display_price` in the requested currency; fixed/from/range amounts use currency minor-unit
exponents and HalfUp rounding. Free, on-request and externally calculated prices have no
amount to convert. Each shipping object containing a converted value carries one `display`
valuation (`currency`, `fxRateId`, `capturedAt`); same-currency reads carry none. A preview
uses the requested-currency value with the same valuation. Private assessed charges remain
source values. Conversion supplies neither applicability nor purchase inputs, a binding quote
or a landed-cost total.

Fresh reads select the latest persisted browsing snapshot at/before valuation time. Search
shares one snapshot across item display, shipping display and FX filters; continuation pins
it. Shipping values current terms independently of historical sale-observation valuation.
Reads batch dependencies and reuse conversions without a live FX call. Missing snapshots or
unavailable FX reads fail with 503; invalid snapshots, required missing quotes and overflow
fail with 500. Existing endpoint FX errors remain; shipping enrichment uses
`SHIPPING_TEMPORARILY_UNAVAILABLE` or `SHIPPING_STORAGE_ERROR`. No fallback quote is invented.

FX capture changes valuation without rewriting terms, observations, revisions or search
documents. Anonymous cache keys include currency/search predicates; bounded TTLs still apply.
Credential-bearing responses and errors are `private, no-store`.

## Shipping search and saved destinations

Search and saved filters accept `shipping` independently of item-location `geography`:
whole-country destination, optional exact service, inclusive `maximumPerItem` in declared
currency/minor units, and independent `collectionOffered: true`. Predicates are ANDed;
country/service/price must match one nested offer. A definite service with unknown price can
match without a maximum. Only fixed and explicitly denominated free per-item prices satisfy
one; from/range/external/requested prices and other bases cannot establish an exact maximum.

`WHOLE_COUNTRY_PER_ITEM_V1` compares source currency independently of display currency.
Explicit `WHOLE_COUNTRY_PER_ITEM_FX_V1` requires a maximum and treats its currency as the
target, matching the rounded display amount using the pinned snapshot. Neither interpretation
adds shipping-price sorting, cart charges or combined item-plus-shipping totals. Projection
limits/lag mean absence from results is not proof of unavailable shipping.

Omitting `shipping` adds no predicate. Omitted destination or `{"kind":"NONE"}` selects no
country; service/maximum then require a country, while collection alone does not. `NONE`
alone clears a saved shipping predicate; empty/unsupported predicates fail validation.
Search never changes the browsing preference, and its country may differ from personalization.

Saved filters pin private country, reference release, interpretation and maximum currency;
they retain no address identity, postal/street data, provider evidence or preference reference.
Optional `acceptShippingDestination` guards an explicitly authored country against an owned
address or selected preference. The submitted country (including a subdivision's country)
must match; provider-derived facts alone are insufficient. Resolution checks ownership,
current account, `users:read` and evidence validity. Omitting the guard uses the authored
country without a preference lookup. Address/preference edits never move a saved snapshot.
`shipping: null` clears the predicate; account erasure removes private snapshots.

Scheduled matching applies the same listing-`updated` interval to shipping filters as to
every other filter; effective shipping changes make older listings eligible by advancing their
freshness, while FX-only threshold crossings never reopen older listings. Authored dates,
evaluation cutoff and existing candidate/scan limits apply. FX matches retain immutable snapshot ID/time and target maximum separately from item
price valuation. Listing responses expose `userState.searchFilter.shippingMatchValuation`;
feedback responses expose `shippingMatchValuation`. Later FX/filter edits do not rewrite it.
Private snapshots/valuation stay outside public documents, ordinary CDC and logs.
[Matching](events/flow.md#periodic-saved-filter-matching) owns idempotency and notifications.

## Independent assertions

Eligibility, price knowledge, currency, charge basis/components, handling/transit/total
lead times, collection arrangements and actor roles are independent. A price does not prove
an offer; on-request pricing does not imply confirmation-required eligibility. Money uses
minor units; ranges require ordered bounds in one currency. Counts belong to their declared
item/lot/shipment/seller-order basis. Missing basis/counts yield no calculated charge; checked
arithmetic preserves fixed/from/range semantics. Thresholds use only their explicitly scoped
amount and exact currency. Timing never invents dates or combines incompatible bases.

Origins and collection sites are separate explicit assertions. PartyLocation roles,
inventory, object provenance and dimensions establish neither origin nor packed dimensions.
A site never borrows geography from another assertion. Domestic rules require a usable
explicit origin country; country rules do not. Referenced sites must remain eligible.

## Coverage and conservative assessment

Coverage pins its geographic release, applicability, exclusions, completeness and zone order.
Subdivision matching is exact; hierarchy, customs groups and successor areas are not inferred.
Applicability is a conjunction: known false dominates; missing facts remain undetermined.
Without a selected variant, all known variants must agree. Foreign or invalid variant inputs
remain unresolved even for unrestricted rules.

Exclusions precede zones; unresolved exclusions block positives. The first matching zone
wins, and earlier uncertainty blocks later selection. Services are evaluated independently.
One applicable offered service establishes an offer; confirmation/uncertainty and individual
negatives retain their meanings. Only complete applicable coverage can establish a negative
from no matching area. Global prohibition needs no destination. Unknown vocabulary never
becomes permission.

### Postal interpretation

`POSTAL_RULE_V1` uses geography's country-scoped conservative exact comparison. Reviewed
German prefix/range forms require `DE`: prefixes are 1–4 ASCII digits, inclusive range bounds
are ordered 5-digit strings, and matched codes must have 5 digits. Leading zeros survive;
malformed shapes remain undetermined. These sets assert neither postcode existence nor
deliverability. Other prefix/range forms need explicit review; arbitrary regex is unsupported.

## Incremental typed imports

External SOURCE producers apply only to non-native sources; Shopify/WooCommerce producers
use their enabled method. Acquisition and provider mapping stay external. Failed, partial
or unsupported observations produce no writes; only complete successful scope reads justify
explicit removals. Create profiles before assignment and retry each failed record independently.
Native app/extension implementations follow separately from the backend contract.

`objectKey` identifies a profile in its source/producer/integration namespace; `requestKey`
identifies an exact operation. Creation also retains `creationKey`. Recover profile IDs via
the profile list's object-key/integration filter, including retired/manual profiles. Exact
committed retries return current state for 90 days without repeated effects; changed-key reuse
conflicts permanently. Authority is rechecked before replay, including asynchronous receipts.
Hashed identity/request fences survive source deletion and replay expiry; no raw request body
or reconciliation session is retained.

Ownership covers whole profile/default, listing selection, rate inputs, shipping override
and collection override independently. Imports may claim absent assertions and maintain/remove
their own. Explicit manual assertions, including equal values and clears before any import,
protect the component. Ownership handoff does not fabricate semantic changes or renew evidence.
Implicit source-default selection accompanying another edit claims no selection ownership.
Reconnection requires a new acquisition to reconcile previous imports; it cannot reclaim manual
components. Integration replacement requires deliberate removal of old scoped assertions.

Created or semantically replaced assertions require explicit `observation: {}` with successful
`acquiredAt`; explicit removal does not renew observation. Acquisition must follow the latest
authority boundary and cannot be future-dated. A new acquisition of unchanged terms can update
observation time without semantic revisions or domain events; as newly accepted evidence it
advances the owner's freshness (the profile's, or the listing's together with a shipping
enrichment event). Default observations have no exposed freshness of their own. Evidence-only corrections cannot carry an
observation. Retries/lifecycle commands/ordinary no-ops never renew acquisition. Ordering uses
PostgreSQL microseconds plus increasing producer sequence per component/source lifecycle;
new request keys cannot turn the same acquisition into a new observation.

Source deletion, operator/configuration changes, grant/member revocation and user/Partnership
authority changes conservatively invalidate source imports. Readers immediately exclude stale
imports, preserve independent manual overrides and block fallback from revoked membership.
Regrant/reenabling never revives old imports. The source authority revision is internal; producers
must refetch/reconcile actual state after conflicts, never attach a fresh revision/timestamp to
old payloads. Discovery converges asynchronously. Evidence reports the oldest retained successful
component acquisition; its age alone has no eligibility effect.

| Outcome | Producer action |
| --- | --- |
| Synchronous success | Record committed state; search visibility remains eventual. |
| Async admission | Record queue acceptance; execution still checks authority/dependencies. |
| `SHIPPING_IMPORT_DEPENDENCY_MISSING` | Retry after the catalog listing exists; no product is created/restored. |
| Missing/ineligible profile | Create or restore the owned profile, then reconcile assignment. |
| `SHIPPING_IMPORT_IDENTITY_EXISTS` | Resolve the object key and reconcile the existing profile. |
| Other import/revision conflict | Refetch and reconcile or discard superseded acquisition. |
| 503/lost response | Retry identical content/keys; a missing response does not prove rollback. |

[Canonical import fixtures](fixtures/shipping-imports.json) exercise API and queue contracts.
[Domain fixtures/tests](../src/shipping-core/src/tests.rs) include the shared gallery profile:
painting shipping changes from EUR 80 to EUR 95 (8000 to 9500 minor units) while cabinet prohibition and
independent collection retain their meanings.

## Deterministic crawler item assertions

The pure [normalizer](../src/product-listing-normalization/src/shipping.rs) recognizes bounded,
whole item clauses in the catalog languages. Missing/unsupported/conditional/contradictory
text produces no update. It infers no rates from unscoped amounts, no origin/class from item
facts and no override removal from missing pages. Collection-only item assertions replace
shipping with `PROHIBITED` while preserving independent collection and other components.

Explicit worldwide requested/externally calculated prices remain partial assertions with
unknown eligibility/currency/basis unless separately stated. Dispatch durations mean handling,
never transit/arrival; duration alone cannot identify a service/destination and produces no
write. Whole-component replacements never merge into shared profile services. Clause matching
normalizes layout/case/punctuation/digit forms without translation; unsupported delivery or
conditional clauses veto recognized assertions even across languages.

Only non-native sources receive crawler SOURCE overrides. Application retains the catalog
transaction's optimistic version, raw-revision request identity and original acquisition time;
sequence is acquisition time in Unix microseconds. Historical captures without acquisition
cannot establish observations. [Extraction](crawler/extraction-and-review.md#extraction-contract)
owns timestamp custody and deduplication; unchanged captures do not renew shipping observations.

Catalog/raw progress commits before optional shipping. Transient shipping failure gets at
most one identical immediate retry; conflicts never refetch a revision for old content.
A completed/duplicate raw wake-up does not guarantee/repeat shipping application. Diagnostics
contain identifiers and outcome categories. Recovery must be bounded to selected retained
captures and must not fabricate acquisitions or trigger a global rescrape.
