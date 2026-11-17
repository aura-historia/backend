# Content notices

A content notice asks Aura Historia to review something it lists. Anyone can submit one,
without an account:

- a **provider**, the operator of a listed shop, for example to opt out of being listed;
- a **rights holder**, for example a photographer whose image a listing uses;
- anyone **reporting illegal content**, who may stay anonymous;
- anyone **else**, for example a visitor reporting incorrect information.

Notices are the notice-and-action channel in the sense of DSA Art. 16: fully electronic,
with a reasoned explanation, the exact location, the notifier's name and email, and a
good-faith statement. The mechanism allows but does not require identification (Recital 50),
and Art. 16(2)(c) exempts reports of child sexual abuse offences from naming the notifier, so
a report of illegal content may leave out the name and email. A human always decides; a
notice never removes anything by itself.

The `content-notice-core`, `content-notice-service` and `content-notice-postgres` crates own
this bounded context. PostgreSQL is authoritative; there is no projection.

## Submitted content

| Field | Rule |
| --- | --- |
| Submitter role | `PROVIDER`, `RIGHTS_HOLDER`, `ILLEGAL_CONTENT_REPORTER` or `OTHER`. |
| Reason category | `PROVIDER_OPT_OUT`, `RIGHTS_INFRINGEMENT`, `ILLEGAL_CONTENT`, `INCORRECT_INFORMATION` or `OTHER`. Only a `PROVIDER` may choose `PROVIDER_OPT_OUT`. |
| Explanation | Required, except for `PROVIDER_OPT_OUT` where it is optional. |
| Additional URLs | Up to ten absolute `http`/`https` URLs without credentials, none repeated. |
| Name, email, language | The notifier's contact. Name and email are required, except that an `ILLEGAL_CONTENT_REPORTER` may leave out either or both; a notice without both is anonymous. The email is a bare ASCII address without display text, as the email provider requires: an international domain is kept in its punycode form and a non-ASCII local part is rejected. The language selects the notifier's correspondence and is always given. |
| Good-faith statement | Always confirmed. The notice records which wording was confirmed (`GOOD_FAITH_V1`); a changed wording gets a new version and never rewrites old notices. |

Text is trimmed, bounded and free of control characters (an explanation may keep line breaks
and tabs). The domain types own the exact limits; the migration repeats them as constraints.

## Targets and snapshots

A notice targets exactly one of:

- a **listing source** (`LISTING_SOURCE`), the provider as a whole;
- a **product listing** (`PRODUCT_LISTING`). Its listing source is stored too, so a query
  per listing source covers notices of both kinds.

The server resolves the target and stores a **snapshot** of it as it was at submission: the
listing source's name, URL and partner status, and for a listing its title and URL. A notice
is evidence. It must survive a later change, delisting or deletion of its target, so it has
**no foreign keys** to listing sources or product listings: it neither blocks nor cascades
with their deletion ([storage guide](storage.md#retention-deletion-and-expiry)). The target
identifiers may therefore name rows that no longer exist.

## Submission

`POST /v1/content-notices` accepts a notice from anyone; the [OpenAPI operation](swagger.yaml)
`submitContentNotice` is the request and error contract. A supplied bearer credential must be
valid, but the caller's identity is neither stored with the notice nor logged; logs say only
whether the caller was signed in.

- The submission rules are checked before the target is read. A product listing target must be
  publicly visible under the same lifecycle rule as the public listing detail, so a withdrawn
  listing is not found.
- The target is resolved and snapshotted in the transaction that stores the notice, with the
  partner flag under the same rule as the public listing source details.
- The good-faith confirmation records the statement version the submission form shows.
- The submission fingerprint is SHA-256 over a canonical encoding of the submitted content;
  [`submission_fingerprint`](../src/content-notice-service/src/submission_fingerprint.rs) owns
  the encoding, which never changes for stored notices.
- The committed submission is logged without personal data.

## Submission limits

The anonymous endpoint has cheap abuse protection that needs no UI change. The values were
approved by the product owner for addresses, anonymous notices reuse them, and they are named
constants in
[`submission_policy`](../src/content-notice-service/src/submission_policy.rs); there is no
environment configuration. The limits apply per **group**:

- **Address:** a notice with an email address is grouped by the lowercased address, the stored
  `submitter_email_key`. PostgreSQL computes it for the lock, the read and the stored row alike.
  The fingerprint lowercases in Rust; addresses are ASCII, so both agree.
- **Target without address:** a notice without an email address is grouped with the other
  notices without one about the same target (the listing source, or the listing). Notices
  store no IP address, so the target is the only key an anonymous notice offers. It keeps one
  target from being flooded without letting anyone block anonymous notices about other
  targets; notices with an address about the same target are not counted.
- **Window:** rolling, the 24 hours before the submission's `now`, read from a clock port once
  the group is locked. A stored notice is dated with the same `now`: unlike other entities,
  its `created` and initial `updated` come from that clock rather than from the database, so
  the window it was checked against and the time it records agree.
- **Identical resubmission:** the same submission fingerprint in the same group within the
  window stores nothing, commits, and gets the same `202` as a new notice, so client retries and
  double clicks are safe. It does not count against the budget.
- **Budget:** when 10 notices of the group were stored within the window, a further notice
  is rejected with `429 CONTENT_NOTICE_RATE_LIMITED` and `Retry-After` (exposed to browsers
  through CORS): the whole seconds until enough of them leave the window, normally the oldest.
  For a target without address it is always the whole window, because the exact wait would
  tell anyone when others reported the target. The webapp then offers its `mailto:` fallback.
- **No races:** after resolving the target, the submission transaction takes a
  transaction-scoped advisory lock on the group, then checks for a duplicate, then the
  budget, then inserts. Concurrent identical submissions store one notice, and concurrent
  distinct submissions cannot push a group past its budget.
- **Logging:** the outcomes `duplicate` (INFO) and `rate_limited` (WARN) are logged with the
  request and correlation IDs, never with the email, its key or a hash of either. A
  rate-limited request logs only the kind of group, not its target, because the target can
  indicate who complained about what.

The address is unverified, so the budget throttles volume, not people: anyone can use up
another address's budget for a day, and address variants such as plus-addressing get separate
budgets. Likewise anyone can use up a target's anonymous budget for a day; reporters can still
give an email address, which has its own budget. A human reviews every notice, so this is
accepted for phase 1. There is no per-IP limit: handlers have no trustworthy client IP and
notices store none, and the web ACL has no rate rule ([front door](http-api-front-door.md)).

## Receipt email

Every stored notice with an email address gets exactly one receipt email to that address, the
confirmation of receipt that DSA Art. 16(4) expects where the notice contains electronic contact
information. A notice without an email address stores no receipt intent: the notifier cannot be
sent a receipt or, later, the decision (Art. 16(5)). An identical resubmission and a
rate-limited submission store nothing, so they send nothing. There is no separate receipt cap:
the per-address budget already bounds receipts, and every accepted notice is acknowledged.

- **Delivery:** the submission enqueues the receipt through the notification context's
  [external email delivery](events/flow.md#external-email-delivery) in the transaction that
  stores the notice. The notice and its receipt intent commit together or not at all; a failed
  enqueue fails the submission with `503`. The delivery pipeline sends it after commit, with
  its retries, DLQ and alarms. The receipt's origin is the notice (`CONTENT_NOTICE` and the
  notice ID), its template kind `CONTENT_NOTICE_RECEIPT`.
- **Content:** fixed localized text plus data from Aura Historia's side only: the reference
  (the notice ID), the receipt time in UTC, the listing source's name, what the notice covers
  (one listing, with its snapshot title or an "untitled listing" stand-in, or the whole
  provider with all of its listings), and the reason category. Nothing the notifier typed is
  echoed back (no name, explanation or URLs), so a notifier cannot put text of their own into an
  email to a third party. The provider name and listing title are text the listed provider
  published; the templates escape them, and the per-address budget bounds how many receipts one
  address gets. [`ContentNoticeReceipt::for_notice`](../src/notification-core/src/external_email.rs)
  owns that selection.
- **Wording:** it promises no time frame, advertises nothing and links to no account or
  settings page, because the recipient usually has no account. It says that whoever did not
  submit the notice can ignore the email, since the address may belong to someone else. The
  [templates](../mjml/content-notice/receipt) exist in `de`, `en`, `es`, `fr` and `it`; any
  other submitter language gets English. Their design is the `Email · content-notice/receipt`
  artboards ([email templates](../mjml/README.md)).
- **Admin view:** the admin detail shows the receipt's delivery status, attempts and delivery
  time. `FAILED` means the email provider rejected the receipt, for example its address, and
  it will not be retried; admins then answer the notifier manually from the contact mailbox.
  An account-level refusal, such as paused sending, is retried and then dead-lettered instead.
  `PENDING` and `PROCESSING` normally pass within minutes;
  a receipt whose delivery job was dead-lettered, which alarms, keeps either status until the
  job is redriven ([runbook](durable-worker-runbook.md#notification-recovery-limits)).

## States

Every notice starts and, for now, stays `SUBMITTED`. Decision states come with the decision
workflow; until then, admins communicate decisions outside the system.

## Personal data

A notice holds the notifier's name and email when they gave them, and free text and URLs they
typed.

- No IP address or other request metadata is stored.
- Name, email, explanation and additional URLs never appear in logs, metrics, URLs, error
  messages, CDC jobs or projections. The domain types redact them from `Debug` output, and
  the table is not part of CDC selection.
- Admin lists offer no filter on personal data.

Notices are kept until a retention rule exists; retention and erasure come with the decision
workflow, which has to define what an erased notice still proves. The receipt message holds
the notifier's address too; that retention rule must erase it by the notice's origin as well.

The table also stores two support values for the submission rules: the lowercased email
address (absent without an address) and a SHA-256 fingerprint of the submission, which the
submission use case defines.

## Access

Admin reads require the `ADMIN` role; delegated access tokens additionally need the scope
`content-notices:read`. There is no write scope. The use case checks both inside its read
transaction; route protection alone does not authorize. The counts in the admin overview are
not notice reads: they follow the overview's own access rule (`admin-overview:read`).

## Admin review queue

`GET /v1/admin/content-notices` lists notices newest first. Admins find new notices there and in
the counts of the admin overview (`GET /v1/admin/overview`); there is no alert yet.

- Each item is a summary: target, the listing source name, listing title and partner flag from
  the snapshot, role, category, and the submitter's name and email, each omitted when the
  notifier gave none. A notice without both is shown as anonymous; one without an email cannot
  be answered. The explanation and URLs are left to the detail read.
- Filters cover the target kind, a listing source (notices about the source and about its
  listings), a listing, the category, the role and the creation time. There is no filter on
  personal data and none on the state, which is always `SUBMITTED` until decisions exist.
- Pagination is a keyset over `(created, content_notice_id)`, both in the requested direction,
  read in one query per page. The cursor carries the last position and a binding of the filters
  and sort; a cursor presented with other filters or another sort is rejected.
- Responses are never cached.

The [OpenAPI](swagger.yaml) operation `adminSearchContentNotices` owns the exact parameters and
errors.

## Admin detail

`GET /v1/admin/content-notices/{contentNoticeId}` returns one notice with everything an admin
needs to handle it: the submitted content, the snapshot, and the target as it is now.

- The snapshot never changes. The current target is read live in the same query as the notice:
  whether the listing source still exists, its name, slug, ingestion methods and partner status
  (same rule as at submission), and for a listing target whether the listing exists and its
  lifecycle. A target that no longer exists is reported as such; the notice is still returned.
- The [receipt](#receipt-email) delivery is read in the same query, found by its origin. It is
  omitted only when no receipt was enqueued, which is always the case for a notice without an
  email address.
- A stored notice that breaks an invariant, or an unknown stored value, fails the read instead of
  being defaulted.
- Responses are never cached.

The [OpenAPI](swagger.yaml) operation `adminGetContentNotice` owns the response shape and errors.

## References

- Schema: [migration](../migrations/20261114090000_content_notices_and_external_email.sql).
- Public contract: [OpenAPI](swagger.yaml) and the [changelog](CHANGELOG.md).
- Object ID prefix `ntc`: [object IDs](object-ids.md).
