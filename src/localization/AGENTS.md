# DOX

## Purpose

- Own pure reusable language and localization values.

## Core Design

- `Language` and `Localized<L, T>` live here. `Language::as_str` and exact `from_code` own canonical short language codes; HTTP aliases stay at the API boundary.
- No DTO, record, document, SQL, HTTP, AWS, OpenSearch, or environment code.

## Ownership

- Parent doc: `src/AGENTS.md`.

## Verification

- `cargo check -p localization --all-targets --all-features`
- `cargo test -p localization --all-features`
