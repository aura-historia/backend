# DOX

## Purpose

- Own `partner-shop-application` crate.

## Core Design

- Partner shop application domain and persistence.
- Root modules: `core`, `data`, `dynamodb`, `service`.
- Main neighbors: `common`, `geo`, `shop`.
- Library crate. Keep domain, persistence, and service seams explicit.
- Shops can apply to become partner: manage shop data and integrate for publishing products.

## Ownership

- This doc rule `src/partner-shop-application/**`.
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

- `cargo check -p partner-shop-application`
- `cargo test -p partner-shop-application --all-features`

## Child DOX Index

- None.
