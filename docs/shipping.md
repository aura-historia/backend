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
needs no origin; domestic comparison requires a usable origin country from the explicitly
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
shared gallery profile: painting inherits EUR 8000 per item and appointment collection;
cabinet inherits the profile with a shipping prohibition. A profile change to EUR 9500
changes painting shipping while cabinet shipping and independently applicable collection
retain their meanings.

The `shipping-core/test-data` [fixture builders](../src/shipping-core/src/fixtures.rs) and
[synthetic gallery JSON](../src/shipping-core/fixtures/gallery.json) are reusable by
normalization, provider mapping, projection and API tests. They use fixed canonical object
IDs, explicit reference releases and minor-unit money, with no buyer data. The JSON is
illustrative fixture vocabulary; adapters own their persistence and transport formats.
