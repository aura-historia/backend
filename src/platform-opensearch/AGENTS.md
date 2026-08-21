# DOX

## Purpose

- Own shared OpenSearch protocol mechanics proven across canonical adapters.

## Core Design

- Own generic wire envelopes only: search response metadata, hits, and timeout error.
- Never own bounded-context documents, queries, mappings, or adapter behavior.
- Public protocol types are for production adapter boundaries; document type `T` stays adapter-owned.

## Ownership

- This doc rules `src/platform-opensearch/**`.
- Parent doc: `src/AGENTS.md`.

## Work Guidance

- Keep crate narrow. Add protocol shape only after more than one canonical consumer proves need.
- Do not depend on `common`, core, service, adapter, runtime, or transport crates.

## Verification

- `cargo check -p platform-opensearch --all-targets --all-features`
- `cargo test -p platform-opensearch --all-features`

## Child DOX Index

- None.
