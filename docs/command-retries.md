# HTTP command retries

Retry guarantees belong to the operation contract in [OpenAPI](swagger.yaml).
Forwarding an `Idempotency-Key` header does not establish deduplication or recovery.
Provider, ingestion and protocol workflows retain their owning retry contracts.

## Explicit keyless creation exceptions

| Operation | Owner | Retry policy |
| --- | --- | --- |
| `POST /api/v1/me/addresses` | User address book | Ordinary creation; every accepted request assigns a new address ID. |
| `POST /api/v1/parties/{party_id}/locations` | Party locations | Ordinary creation assigns a new location ID; relocation uses the old location's expected revision. |

These interactive management operations accept equal content as distinct resources.
They ignore legacy `Idempotency-Key` headers, including invalid or repeated values,
without echoing them. Reusing a key provides no deduplication or result recovery.
Shared retry middleware and client helpers must preserve these exceptions and must
not introduce creation receipts or content uniqueness.

Automatic creation retries can produce duplicates if the first request commits and
its response is lost. After an ambiguous timeout, refresh current state and reconcile
before attempting another create. Address/label equality cannot conclusively identify
the original request; a client must not infer success from equal content alone.

Relocation retires the old location and creates its replacement in one transaction.
A repeat using the committed relocation's old expected revision conflicts without
creating a second replacement. It cannot recover the successful result. Refresh the
location collection and reconcile the retired site and replacement before continuing.

[Geography](geography.md) owns authorization, revisions, geographic resolution and
private-content erasure. [Infrastructure](infra.md#removing-geography-creation-receipts)
owns schema cleanup order and rollback compatibility.
