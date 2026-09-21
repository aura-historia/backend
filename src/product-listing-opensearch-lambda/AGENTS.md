# DOX

## Purpose

- Own native Lambda SQS adapter for the ProductListing OpenSearch projection.
- Map Lambda SQS records to the worker's compact schema-2 job contract and service-owned projection use case.

## Core Design

- PostgreSQL owns ProductListing truth. OpenSearch holds the rebuildable external-versioned projection.
- Lambda/SQS DTOs stay here. `aura-historia-worker` owns strict compact-job validation and job disposition.
- Only completed projection results leave a message out of `batchItemFailures`. Retry, poison, timeout, panic, cancellation, unknown effects, and records skipped for exhausted invocation budget remain for source retry/DLQ.
- The deployed mapping uses one source record per invocation. Handler batches share one runtime-deadline budget: each started record gets at most 40 seconds and later records with no remaining budget fail by message ID. It has no receipt daemon, health server, signal loop, custom visibility update, purge, or redrive permission.
- Lambda bootstrap owns config, pool, client, logs, and dependency composition. The service owns projection behavior.
- Source tests drive Lambda SQS records through the worker's real wire decoder and disposition mapping; unit fakes select service outcomes, not transport behavior.

## Ownership

- This doc rules `src/product-listing-opensearch-lambda/**`.
- Parent: `src/AGENTS.md`.

## Local Contracts

- Required env: `POSTGRES_HOST`, `POSTGRES_PORT`, `POSTGRES_DATABASE`, `POSTGRES_MAX_CONNECTIONS`, `POSTGRES_TLS_ROOT_CERT`, `STAGE`, `OPENSEARCH_ENDPOINT_URL`; real stages also need `POSTGRES_SECRET_ARN`, `OPENSEARCH_USERNAME`, and `OPENSEARCH_PASSWORD`, while ephemeral uses fixture `POSTGRES_USERNAME` and `POSTGRES_PASSWORD`. Each invocation refreshes `AWSCURRENT`; a version change rebuilds one full projection handler lease without changing its C08 processing budget.
- Queue is `aura-worker-product-listing-opensearch-<stage>`, with paired native SQS DLQ. Lambda async-invocation DLQ is not used.
- Queue visibility is 300s to preserve the retained native consumer contract while the Lambda handoff remains explicitly gated.
- Additive schema-2 fields stay compatible. Schema 1, malformed or forged jobs stay failed for source redrive.

## Verification

- `cargo check --locked -p product-listing-opensearch-lambda`
- `cargo test --locked -p product-listing-opensearch-lambda --all-features`

## Child DOX Index

- None.
