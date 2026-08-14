# DOX

## Purpose

- Own legacy `product` crate.

## Core Design

- Legacy Product domain, repositories, and core product services.
- Canonical migration types now live in `product-core`, `product-service`, and `product-postgres`.
- DynamoDB/OpenSearch and old service paths remain here until caller cutover.
- Root modules: `core`, `data`, `dynamodb`, `opensearch`, `postgres`, `service`.
- Main neighbors: `common`, `geo`, `shop`.
- Library crate. Keep old behavior stable while migration moves canonical code out.

## Ownership

- This doc rule `src/product/**`.
- Parent doc: `src/AGENTS.md`.
- No child doc below.

## Local Contracts

- Read `AGENTS.md`, `src/AGENTS.md`, then here, before edit.
- New doc only for child crate. No module doc.
- Update this file when legacy crate contract, route/event shape, env vars, or child index change.
- Do not add new canonical migration contracts here.

## Work Guidance

- Think caveman. Talk caveman. Few word.
- Service and repository split stay clean.
- Keep transport and runtime glue out of domain core.

## Verification

- `cargo check -p product`
- `cargo test -p product --all-features`

## Child DOX Index

- None.
