# Content notices

A content notice asks Aura Historia to review something it lists. Anyone can submit one,
without an account:

- a **provider**, the operator of a listed shop, for example to opt out of being listed;
- a **rights holder**, for example a photographer whose image a listing uses;
- anyone **reporting illegal content**.

Notices are the notice-and-action channel in the sense of DSA Art. 16: fully electronic,
with a reasoned explanation, the exact location, the notifier's name and email, and a
good-faith statement. A human always decides; a notice never removes anything by itself.

The `content-notice-core`, `content-notice-service` and `content-notice-postgres` crates own
this bounded context. PostgreSQL is authoritative; there is no projection.

## Submitted content

| Field | Rule |
| --- | --- |
| Submitter role | `PROVIDER`, `RIGHTS_HOLDER` or `ILLEGAL_CONTENT_REPORTER`. |
| Reason category | `PROVIDER_OPT_OUT`, `RIGHTS_INFRINGEMENT`, `ILLEGAL_CONTENT`, `INCORRECT_INFORMATION` or `OTHER`. Only a `PROVIDER` may choose `PROVIDER_OPT_OUT`. |
| Explanation | Required, except for `PROVIDER_OPT_OUT` where it is optional. |
| Additional URLs | Up to ten absolute `http`/`https` URLs without credentials, none repeated. |
| Name, email, language | The notifier's contact. The email is a bare address without display text; the language selects the notifier's correspondence. |
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
valid, but the caller's identity is not stored with the notice.

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
approved by the product owner and are named constants in
[`submission_policy`](../src/content-notice-service/src/submission_policy.rs); there is no
environment configuration.

- **Address:** submissions are grouped by the lowercased email address, the stored
  `submitter_email_key`. PostgreSQL computes it for the lock, the read and the stored row alike.
- **Window:** rolling, the 24 hours before the submission's `now`, read from a clock port.
- **Identical resubmission:** the same submission fingerprint from the same address within the
  window stores nothing, commits, and gets the same `202` as a new notice, so client retries and
  double clicks are safe. It does not count against the budget.
- **Budget:** when 10 notices from the address were stored within the window, a further notice
  is rejected with `429 CONTENT_NOTICE_RATE_LIMITED` and `Retry-After`: the whole seconds until
  enough of them leave the window, normally the oldest. The webapp then offers its `mailto:`
  fallback. The budget also bounds receipts per address.
- **No races:** after resolving the target, the submission transaction takes a
  transaction-scoped advisory lock on the address key, then checks for a duplicate, then the
  budget, then inserts. Concurrent identical submissions store one notice, and concurrent
  distinct submissions cannot push an address past its budget.
- **Logging:** the outcomes `duplicate` (INFO) and `rate_limited` (WARN) are logged with the
  request and correlation IDs, never with the email, its key or a hash of either. A
  rate-limited request does not log its target either, because it can indicate who complained
  about what.

There is no per-IP limit: handlers have no trustworthy client IP and notices store none. The
web ACL has no rule for it either ([front door](http-api-front-door.md)).

## States

Every notice starts and, for now, stays `SUBMITTED`. Decision states come with the decision
workflow; until then, admins communicate decisions outside the system.

## Personal data

A notice holds the notifier's name and email, and free text and URLs they typed.

- No IP address or other request metadata is stored.
- Name, email, explanation and additional URLs never appear in logs, metrics, URLs, error
  messages, CDC jobs or projections. The domain types redact them from `Debug` output, and
  the table is not part of CDC selection.
- Admin lists offer no filter on personal data.

Notices are kept until a retention rule exists; retention and erasure come with the decision
workflow, which has to define what an erased notice still proves.

The table also stores two support values for the submission rules: the lowercased email
address and a SHA-256 fingerprint of the submission, which the submission use case defines.

## Access

Admin reads require the `ADMIN` role; delegated access tokens additionally need the scope
`content-notices:read`. There is no write scope. The use case checks both inside its read
transaction; route protection alone does not authorize.

## Admin review queue

`GET /v1/admin/content-notices` lists notices newest first. Admins find new notices there and in
the overview counts; there is no alert yet.

- Each item is a summary: target, the listing source name, listing title and partner flag from
  the snapshot, role, category, and the submitter's name and email. The explanation and URLs are
  left to the detail read.
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
- A stored notice that breaks an invariant, or an unknown stored value, fails the read instead of
  being defaulted.
- Responses are never cached.

The [OpenAPI](swagger.yaml) operation `adminGetContentNotice` owns the response shape and errors.

## References

- Schema: [`content_notices` migration](../migrations/20261114090000_content_notices.sql)
  and the [scope migration](../migrations/20261114090100_content_notices_read_scope.sql).
- Public contract: [OpenAPI](swagger.yaml) and the [changelog](CHANGELOG.md).
- Object ID prefix `ntc`: [object IDs](object-ids.md).
