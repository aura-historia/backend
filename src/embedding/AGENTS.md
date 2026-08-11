# DOX

## Purpose

- Own product-neutral embedding capability and Vertex AI adapter.
- Expose typed text/image requests, typed 768-value embeddings, mockable port, and guarded external image fetch capability.

## Core Design

- `EmbeddingGenerator` is product/search-filter free. Callers compose their own text.
- `VertexAiEmbeddingGenerator` owns Vertex HTTP, optional-image fetch/sniff/retry, and query LRU cache.
- `SafeImageFetcher` is reusable for adapter-owned multimodal inputs. Image fetches allow only HTTP(S) targets resolving solely to public IPs, recheck every redirect, bound response time and bytes, and sniff JPEG, PNG, GIF, WebP, HEIC, or HEIF bytes.
- Provider DTOs stay private. This crate logs no input, response, token, URL, or error payload.
- Composition roots pass `VertexAiEmbeddingConfig` plus Google access-token credentials; they resolve ADC at that boundary. This crate reads no environment variables directly.
- `EmbeddingGenerator` is the embedding port. `SafeImageFetcher` is a product-neutral adapter safety capability; services do not depend on it.

## Ownership

- This doc rules `src/embedding/**`.
- Parent doc: `src/AGENTS.md`.

## Verification

- `cargo fmt --all -- --check`
- `cargo check -p embedding`
- `cargo test -p embedding --all-features`
