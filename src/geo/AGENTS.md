# DOX

## Purpose

- Own `geo` crate.

## Core Design

- Geo domain, data, and persistence across DynamoDB and OpenSearch.
- `Geocoder` is shared product-neutral geocoding port. `GoogleGeocoder` owns Google HTTP and private DTOs.
- Composition root gives `GoogleGeocoderConfig`; crate reads no environment.
- Legacy modules stay: `core`, `dynamodb`, `opensearch`, `data`, `service`.
- Library crate. Keep domain, adapter, and legacy seams explicit.

## Ownership

- This doc rule `src/geo/**`.
- Parent doc: `src/AGENTS.md`.
- No child doc below.

## Local Contracts

- Read `AGENTS.md`, `src/AGENTS.md`, then here, before edit.
- New doc only for child crate. No module doc.
- Update this file when crate contract, route/event shape, env vars, or child index change.
- Keep business rules here, not leaked into callers.

## Work Guidance

- Think caveman. Talk caveman. Few word.
- Service and repository split stay clean.
- Keep transport and runtime glue out of domain core.

## Verification

- `cargo check -p geo`
- `cargo test -p geo --all-features`

## Child DOX Index

- None.
