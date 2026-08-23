# DOX

## Purpose

- Own product-neutral embedding capability and Vertex AI adapter.
- Expose typed text/image requests, typed 768-value embeddings, and mockable port.

## Core Design

- `EmbeddingGenerator` exposes semantic product and search-query embedding methods. Callers supply title, optional extra text, and optional image URL fields.
- `VertexAiEmbeddingGenerator` owns Vertex HTTP, Google-specific prompt format, optional-image use, and query LRU cache.
- `image-fetcher` owns reusable guarded image retrieval, including target/redirect validation, retries, byte limits, and media sniffing.
- Provider DTOs stay private. This crate logs no input, response, token, URL, or error payload.
- Composition roots pass `VertexAiEmbeddingConfig` plus Google access-token credentials; they resolve ADC at that boundary. This crate reads no environment variables directly.
- `EmbeddingGenerator` is the embedding port. Services do not depend on `image-fetcher` directly.

## Ownership

- This doc rules `src/embedding/**`.
- Parent doc: `src/AGENTS.md`.

## Verification

- `cargo fmt --all -- --check`
- `cargo check -p embedding`
- `cargo test -p embedding --all-features`
