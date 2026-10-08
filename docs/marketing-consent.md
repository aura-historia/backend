# Email-marketing consent

This document owns consent semantics and integration-specific safety/recovery rules.
[OpenAPI](swagger.yaml) owns HTTP shapes and errors; [event flow](events/flow.md) owns
CDC routing; [infrastructure](infra.md) owns deployment. Schema, state-machine mechanics,
payloads and configuration names belong in code, migrations and tests.

## Authority and scope

- Consent is for Aura's email-marketing purpose and an **exact address**. PostgreSQL
  owns local decisions; Loops owns its preferences, suppression and final sending gate.
  A local true value is not proof of provider eligibility or delivery.
- The account email is fixed at registration. Provider identity/profile refreshes do
  not update it or transfer consent. Email-only subscriptions do not create Users;
  never convert an orphaned User grant into an anonymous one.
- Profile updates do not grant consent or change provider subscription/list membership.
  No all-User boolean scan may resubscribe contacts. This integration does not authorize
  an independent SES marketing sender; SES sends the confirmation proof, not newsletters.

## Grants and double opt-in

Native signup may carry immutable `custom:marketing_consent` with exactly `"true"` or
`"false"`; absence requests nothing. It is signup intent, not current consent or an
identity claim. A grant requires trusted `PostConfirmation_ConfirmSignUp`, verified email
and a newly created User. Federated/profile attributes, malformed values, password-reset
triggers and registration replays cannot grant or restore withdrawn permission.

Newsletter requests **always** require Aura double opt-in, even for an authenticated
account's email. Authentication supplies optional profile fallback, not mailbox proof.
Persist the bounded, exact-target challenge before sending; keep only the token digest,
not raw tokens or rendered mail. No database transaction spans email I/O. Distinguish
rejection from ambiguous acceptance without leaking account/challenge existence.

Only explicit POST confirmation consumes proof; GET, redirects and link prefetch do not.
Check expiry on use. Confirmation, local decision and synchronization intent commit
together; replay confirms the original outcome rather than restoring a later withdrawal.
A successful confirmation is not confirmation of Loops delivery.

## Synchronization and provider eligibility

Local decisions and immutable-target intents commit together, deduplicated by stable
proof/action identity. Serialize recipient work and use a separate consent-decision fence:
a profile write or unchanged boolean must not erase decision ordering. Expired grants
cannot run later; revokes must not expire into permission to send.

The worker claims the **specific** queued intent in a short transaction, rechecks the
latest decision/address ownership before provider I/O, and rechecks/finalizes under the
same lease afterward. Active claims, missing intents and uncertain commits do not
acknowledge. If a grant races withdrawal, the coordinator must durably confirm or schedule
corrective revoke work before acknowledging—even when retry finds the original grant
already `BLOCKED`. Compensation must not overwrite a newer grant or a reassigned address.

Loops contacts are addressed by exact email, without plus-tag/dot folding or provider
`userId` lookup. Aura metadata is not identity. Use the same configured single-purpose
list **ID** for writing and back-sync; do not change unrelated lists. A missing contact
is already revoked: do not create one merely to unsubscribe.

A successful update response is not eligibility. Grants require consistent contact,
global subscription/provider opt-in, target-list and suppression observations. Missing,
contradictory or uncertain readback is not success. Respect Preference Center opt-outs,
bounce/complaint suppression and deleted contacts; never bypass them with another identity,
list or suppression-removal API. See [Loops preferences](https://loops.so/docs/contacts/mailing-lists)
and [suppression](https://loops.so/docs/contacts/suppression).

Definite pre-write failures can release the matching claim for retry; potentially accepted
writes require reconciliation, not automatic HTTP-client replay. An abandoned ambiguous
grant reads provider state instead of blindly subscribing again. Valid observations that
do not establish eligibility can commit `BLOCKED`; unavailable, malformed or contradictory
reconciliation reads remain retryable and must not acknowledge. Exact finalization receipts
resolve lost replies without another grant.
[Sync service](../src/user-service/src/use_cases/commands/sync_marketing_consent_intent.rs)
and [Loops adapter tests](../src/user-loops/src/marketing_email_consent_writer.rs) own the mechanics.

## Preference webhooks

Authenticate the exact received bytes before parsing. A success response requires an
atomic committed application/receipt, matching duplicate or safe ignored disposition.
Conflicting reuse of a delivery ID is not success; no detached application follows ACK.

Configure the signed Loops endpoint for these supported observations:

| Provider event | Meaning |
| --- | --- |
| `contact.unsubscribed`, `email.unsubscribed` | Global withdrawal |
| `contact.deleted` | Provider contact removal, not a user click |
| `contact.mailingList.unsubscribed` | Withdrawal only for the configured purpose list |
| `email.spamReported` | Provider block, not invented user consent evidence |
| `email.hardBounced` | Delivery signal; suppression still gates sending |
| `contact.mailingList.subscribed` | Observation/possible API echo; never grants |
| `email.resubscribed` | Grant to an existing exact-mailbox User only after fresh provider eligibility and unchanged local fences |

Use provider event time for ordering, delivery identity for deduplication, and receipt
time for processing/expiry. At provider timestamp precision, equal-time negatives win;
positives must be strictly newer than provider/local decisions. Future-dated positives
cannot advance the fence. Bind current contact identity to the exact mailbox; custom
Aura metadata cannot redirect an event to another User. Anonymous positives are receipted
and replay-marked as ignored, never turned into new Users or email-only grants. Anonymous
withdrawals still cancel matching pending grants and confirmation proofs.

Withdrawal, decision revision, obsolete-grant cancellation, pending-proof invalidation
and receipt commit atomically, without an ordinary outbound echo. Detailed receipts
are bounded; mailbox ordering/contact fences and minimal positive-delivery replay markers
survive cleanup. Otherwise an old or previously ignored positive could become a new grant.
Provider delivery is finite and reads are not atomic with local commits: this is conservative
protection, not instantaneous consistency. [Application tests](../src/user-service/src/use_cases/commands/apply_loops_preference_event.rs)
own event mapping and race cases.

## Evidence and privacy

After confirmed commit, API/PostConfirmation paths emit `marketing_consent.evidence.v1`.
The [emitter](../src/user-service/src/use_cases/commands/marketing_consent_evidence.rs) defines
fields. This is best-effort secondary evidence, not an audit database or legal certification:
commit/log delivery is not atomic, gaps/duplicates are possible, and CloudWatch is mutable.

For English signup/DOI grants, `backend:email-marketing-purpose:v1` maps to this frozen wording:

> I would like to receive email from Aura Historia with newsletters, personalized product recommendations, and information about Aura Historia features and plans. I can unsubscribe at any time.

This backend reference does **not** prove which frontend text a user saw. Other/unobserved
wording uses `not-recorded` / `und`; do not fabricate translations or reuse signup copy
as withdrawal evidence. Purpose/copy binding changes need a reviewed version and verified
frontend mapping, not client-provided wording.

The API and PostConfirmation evidence groups have no automatic expiry and retained resource
policies. Limit privacy/support lookup to approved subjects, groups and time windows;
select event fields, not full messages. Fingerprints remain personal/pseudonymous data,
not anonymous data or metric dimensions. Review access, minimization, retention and erasure.
CloudWatch erasure is stream/group-level: assess collateral records and coordinate retained
resource ownership. [Log-group adoption](infra.md#manual-operations) is a separate operator action.

## Operations and recovery

Before traffic, verify the shared purpose-list ID, signing configuration/public endpoint,
SES identity and isolated recipient access, pinned confirmation templates and frontend
confirmation URL. Configuration and mocks do not prove live provider acceptance.

Monitor queue age/DLQs, intent outcomes, cleanup and API/webhook latency/errors alongside
Loops retry/failure history and endpoint enablement. No receipt can exist for a pre-commit
failure; an empty DLQ or receipt sample is not proof of healthy synchronization. Exhausted
or disabled webhook delivery needs provider-state reconciliation, never a fabricated event.

Cleanup uses bounded batches: unconfirmed proofs expire after 24 hours; confirmed proofs
retain at least seven days after confirmation and issuance expiry; completed applied/superseded
intents and processed webhook receipts retain at least 120 days. Unfinished/blocked/failed work
and independent replay fences are not housekeeping targets. These are operational windows,
not legal consent history; review them against archive/queue recovery when changing retention.

Before [controlled replay](durable-worker-runbook.md#failure-custody-and-controlled-redrive),
inspect the exact intent, current decision/revision, mailbox ownership, provider eligibility
and corrective work. Never recreate a missing intent from a message, reset applied receipts,
replay an expired/revoked grant or remove fences to force a resubscription. Resolve raced
blocked grants through the guarded repair path rather than declaring them settled by status alone.
