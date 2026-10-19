# Account deletion and provider identities

Self-service and administrator deletion commit account removal, backend credential
invalidation, marketing consent erasure work and a provider deletion request in one
PostgreSQL transaction. Authorization and last-administrator protection apply first.
A failure rolls everything back. An unbound account requires no provider work.
HTTP `204` confirms the business commit; Cognito removal follows asynchronously.

Backend authentication immediately rejects the removed account, including newly issued
Cognito JWTs. Until provider deletion completes, Cognito sign-in may succeed and the
signup address may remain reserved. After removal, the same email can register a new
account with a new subject and user ID. Deleted accounts cannot be recovered. Delayed
registration callbacks for their issuer/subject fail closed. Removing a federated
Cognito profile does not delete the upstream Google/Facebook account.

## Delivery and provider reconciliation

The committed request travels through DMS, Kinesis, the CDC router and the dedicated
`user-identity-deletion` Standard SQS worker. Only the deleted user ID enters that path;
the worker rereads provider identity and progress from PostgreSQL. Identity rows, provider
usernames, emails and profiles are excluded from CDC. Only request inserts produce jobs.

Claims commit before provider I/O and expire after five minutes. Each provider operation
has a three-second deadline and the worker's invocation budget is shorter than its lease.
A resolved username is checkpointed before deletion. On retry, the adapter strongly reads
that exact target and verifies its subject before removal; an already absent target
completes successfully. Lost deletion responses and uncertain completion commits are
therefore recoverable without repeating an eventually consistent discovery lookup.
[Cognito lists are eventually consistent](https://docs.aws.amazon.com/cognito-user-identity-pools/latest/APIReference/API_ListUsers.html):
discovery follows all pages and requests only subject attributes. An empty lookup remains
unfinished and retains its SQS message. Missing requests,
active leases, invalid state and provider failures also remain retryable. Only committed
completion acknowledges a job; completed redelivery skips provider I/O. Claim versions
fence stale workers. No database transaction spans a provider call, and deletion targets
are never selected by email.

Minimal identity and completion fences survive account removal to reject registration
replays and deduplicate worker delivery. Restrict them as identity data; they contain no
email, credentials or profile. Do not purge them as ordinary completed-work receipts or
bind their subjects to another account.

## Operations and recovery

Apply the migration and deploy compatible CDC selection, router and worker before
accepting deletion requests. The worker needs only `ListUsers`, `AdminGetUser` and
`AdminDeleteUser` on its stage's pool. The API needs no provider deletion permission.

Queue age and DLQ alarms cover this scope through the existing worker infrastructure.
After fixing provider, IAM, issuer or network failures, replay retained jobs through the
[worker runbook](durable-worker-runbook.md#user-identity-deletion-recovery). Preserve source
identity, saved targets and fences during recovery or rollback. A provider profile removed
before its username was checkpointed may require exact provider evidence to reconcile;
an empty listing or email match alone does not establish completion.

For accounts deleted before this contract, cascading removal may have destroyed the
binding. Recover the exact user ID, issuer and subject from approved deletion evidence,
verify that no current business binding exists, and insert the retained deletion state
and compact request together through an authorized operational change. Never bulk-delete
apparently unbound Cognito users: confirmation and registration can race. Deployment
procedures belong in [infrastructure](infra.md).
