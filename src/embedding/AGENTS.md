# DOX

## Purpose

- Own reusable external embedding adapters.
- Vertex AI Gemini client stays here; service ports stay in owning service crates.

## Core Design

- `VertexAiEmbeddingClient` owns direct Vertex HTTP and Google ADC access-token use.
- Composition roots pass typed `VertexAiEmbeddingConfig` and credentials. This crate reads no environment variables.
- `VertexAiSearchFilterEmbeddingGenerator` adapts the reusable client to the current search-filter port.
- Future Product embedding ports may wrap `VertexAiEmbeddingClient`; do not couple the client to Product service now.

## Ownership

- This doc rules `src/embedding/**`.
- Parent doc: `src/AGENTS.md`.

## Verification

- `cargo check -p embedding`
- `cargo test -p embedding --all-features`
