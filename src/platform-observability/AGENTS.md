# DOX

## Purpose

- Own tracing subscriber construction from typed configuration.

## Core Design

- Own generic `tracing` setup only.
- Composition roots read `LOG_LEVEL` and pass `LoggingConfig`.
- No product, crawler, LLM, entity, SDK, or environment-read behavior.

## Ownership

- This doc rule `src/platform-observability/**`.
- Parent doc: `src/AGENTS.md`.

## Verification

- `cargo check -p platform-observability --all-targets --all-features`
- `cargo test -p platform-observability --all-features`
