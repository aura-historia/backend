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
`content-notices:read`. There is no write scope.

## References

- Schema: [`content_notices` migration](../migrations/20261114090000_content_notices.sql)
  and the [scope migration](../migrations/20261114090100_content_notices_read_scope.sql).
- Public contract: [OpenAPI](swagger.yaml) and the [changelog](CHANGELOG.md).
- Object ID prefix `ntc`: [object IDs](object-ids.md).
