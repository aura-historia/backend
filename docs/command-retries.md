# Application command retries

This contract owns synchronous command identity and recovery. The [operation inventory](command-retry-policies.json)
classifies every inventoried endpoint, its service owner, result policy and retention. Each matching
[OpenAPI operation](swagger.yaml) carries the same `x-command-retry` policy. Keys never replace current
authentication, capabilities, business authority, uniqueness, quotas or optimistic concurrency.

## Client contract

Generate an unpredictable key before the first attempt and retain it with the logical action. Reuse
that key and unchanged validated intent after a timeout or uncertain response. A new action needs a
new key. Newly opted-in existing routes accept optional keys; omission preserves legacy behavior
without keyed recovery. Address and Party-location creation retain their required keys. Batch,
consent, OAuth and provider-ingress contracts remain independent.

New application keys contain one value of 1–128 ASCII letters, digits or `-_.:`. Native duplicates,
comma-joined values and malformed keys return `400 BAD_HEADER_VALUE` naming `Idempotency-Key`.
Existing ingestion, address and Party-location grammars retain their contracts. Accepted keys are
echoed on opted command responses; pre-service failures need not echo them. Responses are private
and non-cacheable. Request IDs, CORS and other transport metadata are rebuilt for the current request.

| Evidence | Response and recovery |
| --- | --- |
| Matching completed command, current authority valid | Recover the declared result without mutation or provider preparation. |
| Same scoped key, different validated intent | `409 IDEMPOTENCY_MISMATCH`; correct the client action, never blindly retry a changed payload. |
| Active coordinated attempt | `409 IDEMPOTENCY_IN_PROGRESS`; bounded exponential backoff with jitter using the same key. |
| Expired, erased or deliberately non-replayable result | `409 IDEMPOTENCY_RESULT_UNAVAILABLE`; the fence prevents re-execution. Read current state or follow the owning recovery procedure. |
| Receipt storage unavailable or commit uncertain | `503 IDEMPOTENCY_TEMPORARILY_UNAVAILABLE`; retry the same identity. A failed acknowledgement does not prove rollback. |
| Unsupported/corrupt evidence | `500 IDEMPOTENCY_INTERNAL_ERROR`; fail closed and investigate without deleting evidence. |

A genuinely new command still encounters ordinary stale-revision, uniqueness, quota and lifecycle
errors. No guarantee of exactly-once external execution or charging is implied.

## Identity, authorization and transactions

Trusted actor, logical operation and target define the namespace. Direct and delegated credentials
for the same user intentionally share actor identity across refresh; each credential must retain
its current capabilities. Other users and trusted service/system actors have separate namespaces.
Administrator and protected source-default aliases share their logical operation after their
respective current authorization checks. Single-listing aliases share identity; a protected request
must additionally prove that the retained listing belongs to its currently authorized path source.

V1 fingerprints bind validated submitted fields, target IDs, expected revisions, PATCH omission,
null and value distinctions, and transition variants. True sets sort deterministically; meaningful
text/image arrays preserve order. Generated IDs, timestamps, embeddings, live price configuration
and provider responses do not enter submitted-intent fingerprints. Administrator profile/role/tier
updates share one operation and bind the category; approval and rejection likewise share one decision
operation. Different methods representing inverse actions have distinct operations unless explicitly
modelled as one variant-bearing setter.

Services establish current authority before receipt disclosure, then coordinate the key before
new-command quota/CAS checks. Successful changes, events, local delivery intents and the receipt commit
in the same transaction. Successful no-ops record their outcome without manufacturing revisions or
events. Rolled-back commands leave no successful receipt. PostgreSQL advisory acquisition is
non-blocking; identity checks and database uniqueness remain authoritative even under hash collisions.
Owner/grant/aggregate locks retain their bounded platform timeouts. Resource-specific authority locks
precede key coordination where the existing geography contract requires that order.

The stored V1 outcome is a service-owned application codec, with explicit validated mappings, never
an HTTP response, arbitrary provider payload or serialized aggregate row. Adapter envelope versions
and service codecs must remain readable over the promised recovery horizon, including rollback.
Transport middleware only echoes metadata and applies cache policy; it does not save receipts.

## Outcome and erasure policies

| Family | Result after later edits/deletion | Retention |
| --- | --- | --- |
| Ordinary keyed commands | Original application identity/result/revision, including no-op classification. Later GETs return current state. | Snapshot 30 days from commit; permanent successful-command fence. |
| Private user-owned commands | Same original-result rule except the private exceptions below. Account erasure removes private evidence and authentication. | At most 30 days for new snapshots; owner erasure cascades. |
| Saved-filter creation | Original creation after edits; deletion erases the snapshot and retains its fence. | No time expiry while the filter exists; fence until account erasure. |
| Saved-filter edits/feedback | Original result after edits. Filter deletion erases linked snapshots and content fingerprints; replay becomes unavailable. | Snapshot at most 30 days; fence until account erasure. |
| Private address create/update | Same address's current authorized view; no historical address body is recorded. Deleted entries cannot replay or execute again. | Creation recovery has no TTL; update recovery lasts 30 days. Both fences remain until account erasure. Deletion scrubs content fingerprints. |
| Browsing destination | Current authorized preference after a matching completed change; never reapply an older preference. | Receipt snapshot marker 30 days, fence until account erasure. |
| Party-location creation | Original result after edits, under current Party authority. | Existing permanent receipts, without TTL. |
| New location/default mutations | Original protected outcome under current Party/source authority. | Ordinary 30-day snapshot and permanent fence. |
| Administrator erasure | Minimal deleted user ID under current administrator authority; never private account contents. Self-erasure remains authentication-terminal. | 30-day metadata result, permanent fence. |

Address creation's existing deleted-result conflict remains unchanged. New address-update replay after
deletion reports result unavailable. New keyless ordinary requests retain their existing behavior.

Saved-filter create and update check known receipts before embedding preparation, finish that read
transaction, then coordinate/check again in the write transaction. Concurrent initial preparation or
a preparation/commit crash can repeat an embedding call; at most one keyed mutation commits. No
transaction is held across embedding I/O. A completed replay skips preparation, quota consumption and
its own stale-revision checks.

## Billing workflow

Checkout, manage and portal use separate logical operations with owner-scoped durable admission,
frozen V1 inputs and fenced attempts. Checkout/manage share owner exclusion while an operation is
unresolved. Customer identity, email/name, resolved price, chosen checkout/portal branch and redirect
URLs freeze before their corresponding provider calls. Per-step provider keys derive from the trusted
operation identity and step, never a request ID. Provider calls run outside database transactions;
customer creation, association, session creation and finalization have durable checkpoints.

A retry recognizes the operation before the existing-customer guard. A new checkout still rejects an
existing customer; a different key cannot race an unresolved customer-creation operation. A recorded
customer resumes association and session creation. An uncertain provider acceptance retries the same
step and frozen parameters while its provider key remains usable. Attempt leases last 60 seconds;
stale attempts cannot overwrite a newer attempt's checkpoints. A crashed lease may be reclaimed only
for resumable workflows.

Automatic ambiguous-step recovery stops 23 hours after admission. Stripe API v1 may prune keys after
24 hours, and replays the first executed response, including errors; never assume an indefinite
provider fence or interpret an error as proof that nothing happened. See [Stripe's contract](https://docs.stripe.com/api/idempotent_requests).
Beyond that window return result unavailable and reconcile the known provider objects before any
new action. An existing-customer checkout or missing-customer portal decision is terminal for that
admitted operation, releases owner exclusion and does not trigger a provider request. An eligible
new decision after account state changes uses a new key.

Completed checkout URLs replay for at most 23 hours from admission; portal URLs replay for at most
five minutes. Provider/session invalidation may make a URL unusable sooner. Expired URLs never cause
session recreation under an old key. Use a new key for an intentionally new eligible session.
Completed operations’ frozen private inputs and capability URLs become eligible for bounded physical
cleanup after 30 days, independently of the shorter logical URL window. Unresolved checkpoints remain
available for operator reconciliation; expiry never permits automatic provider execution. Account
erasure removes private operation state; execution fences for surviving accounts do not expire.

## Security and protocol decisions

- Access-token and OAuth-client issuance record minimal issued IDs and successful issuance fences, without raw tokens,
  secrets or secret-response snapshots. The original attempt returns the secret once. Matching retry
  returns result unavailable, with the issued ID in the recovery detail while metadata is retained. List credentials/clients using current authority, revoke/delete the
  uncertain issuance, and explicitly issue its replacement with a new key. Never automatically loop
  with new keys after a timeout. No secret-bearing HTTP response is cached.
- Token deletion retains sufficient keyless semantics: IDs are immutable and never reused; deletion
  cannot revoke a replacement credential with a different ID. It returns successful absence. OAuth
  client deletion is similarly irreversible; `404` after an uncertain deletion is terminal absence.
  Revoked credentials cannot authenticate to recover either response; use another current credential.
- Notification deletion is irreversible under owner/ID scope; missing is terminal absence. Single
  notification seen state retains explicit last-write-wins semantics without revision/event/delivery
  effects. Retry only while that choice is still intended; after another choice, read and reconcile.
  These lower-priority setters do not promise recovery of historical seen-state outcomes.
- Suspension/unsuspension are local security transitions, checked by authentication/authorization;
  they do not perform a provider session-revocation call. Keyed receipts prevent a delayed suspension
  retry from reversing a later unsuspension. Current administrator checks remain required.
- Session revocation validates the current target identity and durably admits one provider action after current administrator authorization, recording only its private provider identity reference.
  Known completed retries recover metadata. In-progress requests return in-progress; an interrupted
  or ambiguous provider action is never automatically repeated after its lease expires, since a
  second call could revoke newly established sessions. Return result unavailable, inspect provider
  audit evidence, and use a new key only for an explicitly new revocation decision.
- Self-erasure removes its own authority and private receipts. Authentication failure after an
  uncertain successful erasure is terminal; an old key never bypasses authentication. Administrator
  erasure atomically records its minimal result with the existing consent/deletion transaction.
  Required provider consent withdrawal remains a durable independent intent, not a synchronous
  provider call. See [marketing consent](marketing-consent.md).
- Newsletter request/confirmation retain recipient-scoped consent identity, anti-enumeration,
  cooldown/throttle, challenge and one-time-proof rules. Repeat request follows that workflow;
  confirmation cannot replay a proof or secret. OAuth authorization/code exchange, token exchange,
  introspection and revocation retain their protocol semantics. An application header is ignored and
  never cached by untrusted email/key or bearer material. See [marketing consent](marketing-consent.md)
  and the individual OpenAPI operations.
- Collection listing writes, async admission, notification collection/all writes, bulk token revocation
  and provider webhooks retain their existing contracts. Single-item payloads on a batch route remain
  batch operations. [Ingestion receipts](product-listing.md#asynchronous-ingestion-submission) and
  [geography receipts](geography.md) keep their existing permanence and privacy policies.

## Operations

The scheduled backend cleanup erases up to its configured batch limit of expired ordinary snapshots
and completed external-operation states per invocation, using ordered selection with `FOR UPDATE SKIP LOCKED`.
Unresolved operations retain their reconciliation evidence until resolution or owner erasure.
Logical expiry is checked on every lookup, independently of cleanup progress. Cleanup never deletes
execution fences or applies a TTL to existing Party-location/ingestion receipts. Retention grows with
successful database-local commands and admitted external workflows; anonymous protocol traffic does
not populate these tables.

Keep the provider account and request contract stable while an external operation is unresolved;
credential rotation must retain that account.

Keep operational tables out of CDC/search/public history. Bound keys, targets and state size. Log only
bounded operation/outcome categories through `command.replayed`/`command.retry` and cleanup counts;
never keys, frozen inputs, private snapshots, capability URLs or raw provider payloads.

Deploy migrations before receipt-aware writers. For rollback across the capability boundary, stop
keyed command traffic and restore a compatible reader before resuming; keep all durable evidence.
Never truncate a receipt table to clear a retry error. For an unresolved external operation, reconcile
provider audit evidence and the recorded step before completing/abandoning it through a reviewed
operator repair. Preserve the original key/fingerprint fence; do not blindly clear a scope or invent
a new provider key. Expired unresolved owner exclusions require reconciliation before admitting an
unrelated customer-creation action. Alert on sustained unavailable/internal errors, unresolved billing
operations and cleanup failures using bounded labels, without content-bearing diagnostics.
