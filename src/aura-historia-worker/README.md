# Worker process

`aura-historia-worker` (the image's `worker` entrypoint) runs the
[delivery outbox](../delivery-outbox/README.md) dispatcher and one consumer per job scope on the
[consumer runtime](../platform-worker/README.md), and publishes the host's
[health metrics](#health-metrics). Each scope's module in [`src/scopes`](src/scopes)
decodes its jobs, invokes the service use case and maps every outcome to a queue disposition
(complete, retry or park). Business logic stays in the service crates.

## Scopes

Every [routed worker scope](../aura-historia-jobs/src/scope.rs) and the ProductListing ingestion
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

A `product-translation` job only records a listing's title translation work. Every worker that
consumes the scope also runs a [title translation round](../../docs/product-listing.md#title-translation)
every 15 minutes, which submits pending work to Anthropic's Message Batches API and commits
ended batches; it needs `ANTHROPIC_API_KEY`.

A `product-embedding` job likewise only records a listing's embedding work. Every worker that
consumes the scope runs an [embedding round](../../docs/product-listing.md#embeddings) every
30 minutes, which submits pending work as a Vertex AI batch prediction job and commits ended
jobs; it needs `VERTEX_AI_PROJECT_ID`, `VERTEX_AI_LOCATION` and
`VERTEX_AI_EMBEDDING_BATCH_BUCKET`.

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
| `AURA_HISTORIA_WORKER_METRICS_NAMESPACE` | CloudWatch namespace of the [health metrics](#health-metrics); unset, none are published |
| `AURA_HISTORIA_WORKER_METRICS_BACKUP_MARKER` | With a namespace: the file whose modification time is the last successful backup |
| `AURA_HISTORIA_WORKER_METRICS_DISK_PATH` | With a namespace: a path on the filesystem whose free space is reported |

Each scope also reads its providers' settings (for example
`ANTHROPIC_API_KEY`, `VERTEX_AI_PROJECT_ID`, `LOOPS_API_KEY`, `OPENSEARCH_ENDPOINT_URL`, the
notification email settings and `AURA_HISTORIA_COGNITO_USER_POOL_ID`). Composition reads and
validates them at startup; a missing setting stops the worker and names the scope. The optional
`OPENSEARCH_CA_CERT` (usually `OPENSEARCH_CA_CERT_FILE`) holds the PEM of a private CA that
OpenSearch's certificate chains to, trusted in addition to the public roots. Google
credentials come from the file `GOOGLE_APPLICATION_CREDENTIALS` names. Notification emails are
rendered from the compiled templates under `MAIL_TEMPLATE_DIR`, which the image sets to its
baked-in [templates](../../mjml/README.md); every template is loaded at startup.

If a consumer, the dispatcher, the wake-up listener or the translation or embedding rounds stop
on their own, the worker shuts down every other task and exits non-zero, so the container is restarted
rather than running with a scope that no longer consumes.

## Health metrics

With a namespace configured, the worker publishes these once a minute in one `PutMetricData`
call, without further dimensions; the host's alarms ([monitoring](../../docs/infra.md#monitoring))
watch them from AWS, so they also fire when the host is gone. Nothing else is published: per-scope
figures stay in `ops inspect`.

| Metric | Unit | Value |
| --- | --- | --- |
| `Heartbeat` | Count | 1, every round |
| `PostgresReachable` | None | 1 when PostgreSQL answered a trivial query within 5 s, otherwise 0, every round |
| `OldestJobAgeSeconds`, dimension `Class` = `normal` or `bulk` | Seconds | Age of the class's oldest waiting job (not under a lease, as in `ops inspect`), 0 without one. Bulk is translation, embedding and isolated projection generations; every other scope, including one no consumer knows, and the oldest undispatched outbox snapshot are normal |
| `ParkedJobs` | Count | Jobs in failure custody across every scope, plus outbox snapshots in custody |
| `OldestPendingEmbeddingAgeSeconds` | Seconds | Age of the oldest product embedding work that is neither committed nor parked, pending or in a batch, 0 without one |
| `FreeDiskPercent` | Percent | Space available to unprivileged writers on the disk path's filesystem |
| `SecondsSinceLastBackup` | Seconds | Time since the backup marker was modified; absent while there is no marker |
| `SecondsSinceLastSuccess`, dimension `Job` | Seconds | Time since each file in the job marker directory was modified; the host's scheduled `ops` jobs touch `<job>` after each success |

The queue figures are read in one transaction with a 15 s statement bound of their own, because
they scan the job table, and are published only when they were read within 20 s; a figure that
could not be read is left out, so the alarms see missing data rather than a made-up value. A failed round
logs `worker.health_metrics.*` at WARN and the next round tries again. The host's IAM user may
publish only to the configured namespace ([host access](../../docs/infra.md#host-access)).

## PostgreSQL pool

The worker's fixed pool is one connection per concurrent job plus seven for the two wake-up
listeners, the dispatcher, settlement, the health metrics and the translation and embedding
rounds: 79 with every scope at its default. Override it with `POSTGRES_MAX_CONNECTIONS` only together with the
concurrency.

## Tests

Each scope module's unit tests assert the disposition of every outcome and the redaction of its
logs; [`tests/`](tests) runs each scope's production composition against real PostgreSQL (and
OpenSearch for the projector). [`tests/end_to_end.rs`](tests/end_to_end.rs) runs the worker on
one listing from its ingestion command through normalization, the outbox, the enrichment
workers and the projection into OpenSearch.
