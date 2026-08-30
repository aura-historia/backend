# DOX

## Purpose

- Own shared OpenSearch protocol mechanics proven across canonical adapters.

## Core Design

- Own generic wire envelopes and fully-read response helper: search response metadata, hits, timeout error, and status/body/source preservation.
- Never own bounded-context documents, queries, mappings, or adapter behavior.
- Public protocol types are for production adapter boundaries; document type `T` stays adapter-owned.

## Ownership

- This doc rules `src/platform-opensearch/**`.
- Parent doc: `src/AGENTS.md`.

## Work Guidance

- Keep crate narrow. Add protocol shape only after more than one canonical consumer proves need.
- Do not depend on application, bounded-context, adapter, runtime, or transport crates.

## Verification

- `cargo check -p platform-opensearch --all-targets --all-features`
- `cargo test -p platform-opensearch --all-features`

## Child DOX Index

- None.
