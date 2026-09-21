# DOX

## Purpose

- Own native Lambda SQS adapter for the ProductListing OpenSearch projection.
- Map Lambda SQS records to the worker's compact schema-2 job contract and service-owned projection use case.

## Core Design

- PostgreSQL owns ProductListing truth. OpenSearch holds the rebuildable external-versioned projection.
- Lambda/SQS DTOs stay here. `aura-historia-worker` owns strict compact-job validation and job disposition.
- Only completed projection results leave a message out of `batchItemFailures`. Retry, poison, timeout, panic, cancellation, and unknown effects remain for source retry/DLQ.
- The function handles one source record per invocation. It has no receipt daemon, health server, signal loop, custom visibility update, purge, or redrive permission.
- Lambda bootstrap owns config, pool, client, logs, and dependency composition. The service owns projection behavior.

## Ownership

- This doc rules `src/product-listing-opensearch-lambda/**`.
- Parent: `src/AGENTS.md`.

## Local Contracts

- Required env: `POSTGRES_*`, `POSTGRES_TLS_ROOT_CERT`, `STAGE`, `OPENSEARCH_ENDPOINT_URL`; real stages also need `OPENSEARCH_USERNAME` and `OPENSEARCH_PASSWORD`.
- Queue is `aura-worker-product-listing-opensearch-<stage>`, with paired native SQS DLQ. Lambda async-invocation DLQ is not used.
- Queue visibility is 270s: six times the 45s Lambda timeout plus the zero-second batching window.
- Additive schema-2 fields stay compatible. Schema 1, malformed or forged jobs stay failed for source redrive.

## Verification

- `cargo check --locked -p product-listing-opensearch-lambda`
- `cargo test --locked -p product-listing-opensearch-lambda --all-features`

## Child DOX Index

- None.
