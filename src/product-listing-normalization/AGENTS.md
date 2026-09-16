# DOX

## Purpose

- Own pure generic ProductListing deterministic normalization.

## Core Design

- Modules: availability, price, date-time, text/language, image URLs, source-listing IDs, raw normalization input, and raw-values normalization. Each raw image value contains one URL; source code selects provider-specific candidates before calling this crate.
- Raw input owns generic action, payload-format/version, source payload, raw-values projection, context, typed SHA-256 input hash, and separate provenance. JSON fields are objects and reject embedded NULs in every key or nested string without rewriting values; caps: source payload 1 MiB, raw values 256 KiB, context/provenance 64 KiB, depth 64. Provenance stays outside input hash.
- `ProductListingRawValues` is the current provider-neutral UPSERT raw-values JSON shape. Its persisted discriminator is `1` and it requires `priceFormat`. Mutable generic fields and source-selected attributes use explicit `SET`, `CLEAR`, or `UNCHANGED` patches. It contains no Auction, lot, or Auction metadata fields; those facts are typed-write-only. `ProductListingNormalizationContextV1` owns generic base URL, fallback currency, and language. The synchronous deterministic normalizer resolves current UPSERT values, classifies invalid outcomes as `CandidateData` or `System`, and passes DELETE without decoding raw values or context.
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
