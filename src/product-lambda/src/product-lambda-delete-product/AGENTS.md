# DOX

## Purpose

- Own `product-lambda-delete-product` crate.

## Core Design

- Worker Lambda consumes product lifecycle delete events.
- Deletes product document in OpenSearch.
- Deletes watchlist records and search-filter match records for product.
- Marks SQS message failed when any cleanup step or OpenSearch bulk item fails.

## Ownership

- This doc rule `src/product-lambda/src/product-lambda-delete-product/**`.
- Parent doc: `src/product-lambda/AGENTS.md`.

## Local Contracts

- Read repo, `src/`, product-lambda, then here, before edit.
- If trigger, env var, queue, or side effect changes, update infra too.

## Verification

- `cargo check -p product-lambda-delete-product`
- `cargo test -p product-lambda-delete-product --all-features`
