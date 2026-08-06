# DOX

## Purpose

- Own product-neutral embedding capability and Vertex AI adapter.
- Expose typed text/image requests, typed 768-value embeddings, and mockable port.

## Core Design

- `EmbeddingGenerator` is product/search-filter free. Callers compose their own text.
- `VertexAiEmbeddingGenerator` owns Vertex HTTP, optional-image fetch/sniff/retry, and query LRU cache.
- Provider DTOs stay private. This crate logs no input, response, token, URL, or error payload.
- Composition roots pass `VertexAiEmbeddingConfig` plus Google access-token credentials; they resolve ADC at that boundary. This crate reads no environment variables directly.
- `EmbeddingGenerator` is the only public port. Services inject it directly; no bounded-context adapter bridge.

## Ownership

- This doc rules `src/embedding/**`.
- Parent doc: `src/AGENTS.md`.

## Verification

- `cargo fmt --all -- --check`
- `cargo check -p embedding`
- `cargo test -p embedding --all-features`
