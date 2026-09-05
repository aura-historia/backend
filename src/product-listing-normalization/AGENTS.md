# DOX

## Purpose

- Own pure generic ProductListing deterministic normalization.

## Core Design

- Modules: availability, price, date-time, text/language, image URLs, source-listing IDs.
- Depends only on pure value crates. No SQLx, HTTP client, LLM, queue, runtime config, provider DTO, or logging.
- Source code maps provider payloads before calling this crate.

## Ownership

- This doc rules `src/product-listing-normalization/**`.
- Parent doc: `src/AGENTS.md`.

## Local Contracts

- Read root, `src/AGENTS.md`, then here before edit.
- Update this doc when API, dependency, normalizer, or limit changes.

## Work Guidance

- Think caveman. Talk caveman. Few word.
- Keep functions synchronous, typed, deterministic.
- Do not add application ports or use cases here.
- Never log raw values.

## Verification

- `cargo check -p product-listing-normalization`
- `cargo test -p product-listing-normalization --all-features`

## Child DOX Index

- None.
