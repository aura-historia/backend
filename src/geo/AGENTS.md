# DOX

## Purpose

- Own `geo` crate.

## Core Design

- Geo domain, data, and external geocoding/OpenSearch value mapping.
- `core::address` owns structured-address formatting; `core::distance` owns pure distance values and canonical `DistanceUnit` codes; `core::continent` owns canonical `Continent` codes; OpenSearch owns distance formatting.
- `data::ContinentData` remains a deliberate legacy compatibility boundary for mixed geo data paths; new adapters use canonical `Continent` with local codecs.
- `Geocoder` is a shared product-neutral address resolution port that returns a formatted address. `GoogleGeocoder` owns Google HTTP and private DTOs.
- Composition root gives `GoogleGeocoderConfig`; crate reads no environment.
- Modules stay explicit: `core`, `opensearch`, `data`, `service`.
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
