# DOX

## Purpose

- Own Product title translation LLM adapter.

## Core Design

- Implements Product service `ProductTitleTranslator` with a neutral LLM client and direct `application` error contracts.
- Owns prompt, response schema, and provider-error mapping.
- No queue, CDC, runtime config, SQLx, or transport code.

## Ownership

- This doc rules `src/product-translation-llm/**`.
- Parent doc: `src/AGENTS.md`.

## Verification

- `cargo check -p product-translation-llm`
- `cargo test -p product-translation-llm --all-features`
