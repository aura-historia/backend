# DOX

## Purpose

- Own `crawler` crate.

## Core Design

- Crawler stack for scraping, review, local storage, and LLM-assisted extraction.
- Root modules: `google_llm`, `local_db`, `logging`, `network`, `review`, `scraper`, `service`, `spider`.
- Main neighbors: `common`, `fxrate`, `product`, `shop`.
- Library crate. Keep domain, persistence, and service seams explicit.

## Ownership

- This doc rule `src/crawler/**`.
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

- `cargo check -p crawler`
- `cargo test -p crawler --all-features`

## Child DOX Index

- None.
