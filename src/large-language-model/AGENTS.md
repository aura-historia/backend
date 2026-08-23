# DOX

## Purpose

- Own reusable typed large-language-model invocation capability.

## Core Design

- Owns reusable typed generation contracts plus a Vertex AI Gemini implementation, credentials, per-request timeout, provider-neutral retry advice, standard JSON Schema normalization at the Vertex boundary, response extraction, provider/model/service-tier vocabulary, invocation metrics, and safe invocation logging.
- `LargeLanguageModel` uses a generic output type. Callers own prompt, schema, generation options, response deserialization, and retry policy. Provider configuration owns the concrete provider model, so callers never carry provider model identifiers.
- `StructuredGenerationRequest` carries image URIs; implementations use `image-fetcher` internally and may deduplicate shared URIs for batch calls. The trait exposes no image-fetch mechanism. Batch results retain request order.
- Knows no Product or Search Filter types.
- Fails construction when configured HTTP client creation fails; no fallback client.

## Ownership

- This doc rules `src/large-language-model/**`.
- Parent doc: `src/AGENTS.md`.

## Work Guidance

- Keep provider request and response types private.
- Do not log prompt, media, response, or credential payloads.

## Verification

- `cargo check -p large-language-model`
- `cargo test -p large-language-model --all-features`
