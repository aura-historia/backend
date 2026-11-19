# Worker process

`aura-historia-worker` (the image's `worker` entrypoint) runs the
[delivery outbox](../delivery-outbox/README.md) dispatcher and one consumer per job scope on the
[consumer runtime](../platform-worker/README.md). Each scope's module in [`src/scopes`](src/scopes)
decodes its jobs, invokes the service use case and maps every outcome to a queue disposition
(complete, retry or park). Business logic stays in the service crates.

## Scopes

Every [CDC worker scope](../aura-historia-jobs/src/scope.rs) and the ProductListing ingestion
command FIFO have a consumer. Each unit of concurrency processes one job at a time within the
job timeout:

| Scope | Concurrency | Job timeout |
| --- | ---: | ---: |
| `product-translation` | 15 | 40 s |
| `product-embedding` | 10 | 55 s |
| `product-listing-opensearch` | 10 | 40 s |
| `search-filter-match-notification`, `watchlist-notification`, `product-content-assessment`, `product-listing-normalization`, `notification-delivery` | 5 | 40 s |
| `marketing-consent-sync` (FIFO), `product-listing-ingestion` (FIFO) | 2 | 35 s |
| `geo-resolution`, `listing-geography`, `listing-shipping`, `user-identity-deletion` | 2 | 25 s |

Leases, retry delays and retry budgets come from the
[scope catalogue](../platform-worker-queue-contract/src/job_queue.rs). A projection generation
consumer (`product-listing-opensearch-<g>`) writes only the index `product-listings-<g>`; it is
used to rebuild into an isolated index with `ops reproject-all --generation <g>`.

## Configuration

| Variable | Meaning |
| --- | --- |
| `AURA_HISTORIA_WORKER_SCOPES` | Comma-separated scope names, or `all` (default) |
| `AURA_HISTORIA_WORKER_PROJECTION_GENERATIONS` | Comma-separated isolated projection generations to consume as well |
| `AURA_HISTORIA_WORKER_CONCURRENCY_<SCOPE>` | Concurrency override, for example `..._GEO_RESOLUTION` |
| `AURA_HISTORIA_WORKER_DISPATCHER` | `false` runs no outbox dispatcher in this process; default `true` |
| `AURA_HISTORIA_WORKER_DISPATCH_BATCH_SIZE` | Outbox rows per dispatch transaction; default 100 |
| `AURA_HISTORIA_WORKER_POLL_INTERVAL_MS` | Longest idle wait without a notification; default 5000 |
| `AURA_HISTORIA_WORKER_SHUTDOWN_GRACE_SECONDS` | In-flight completion window on SIGTERM; default 25. The container's stop timeout must exceed it, or jobs are killed instead of released and return only after their lease |

Each scope also reads its providers' settings (for example
`ANTHROPIC_API_KEY`, `VERTEX_AI_PROJECT_ID`, `LOOPS_API_KEY`, `OPENSEARCH_ENDPOINT_URL`, the
notification email settings and `AURA_HISTORIA_COGNITO_USER_POOL_ID`). Composition reads and
validates them at startup; a missing setting stops the worker and names the scope. The optional
`OPENSEARCH_CA_CERT` (usually `OPENSEARCH_CA_CERT_FILE`) holds the PEM of a private CA that
OpenSearch's certificate chains to, trusted in addition to the public roots. Google
credentials come from the file `GOOGLE_APPLICATION_CREDENTIALS` names. Notification emails are
rendered from the compiled templates under `MAIL_TEMPLATE_DIR`, which the image sets to its
baked-in [templates](../../mjml/README.md); every template is loaded at startup.

If a consumer, the dispatcher or the wake-up listener stops on its own, the worker shuts down
every other task and exits non-zero, so the container is restarted rather than running with a
scope that no longer consumes.

## PostgreSQL pool

The worker's fixed pool is one connection per concurrent job plus four for the wake-up listeners,
the dispatcher and settlement: 76 with every scope at its default. Override it with `POSTGRES_MAX_CONNECTIONS` only together with the
concurrency.

## Tests

Each scope module's unit tests assert the disposition of every outcome and the redaction of its
logs; [`tests/`](tests) runs each scope's production composition against real PostgreSQL (and
OpenSearch for the projector). [`tests/end_to_end.rs`](tests/end_to_end.rs) runs the worker on
one listing from its ingestion command through normalization, the outbox, the enrichment
workers and the projection into OpenSearch.
