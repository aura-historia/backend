# DOX

## Purpose

- Own canonical credential identifiers and scope vocabulary.

## Core Design

- Own OAuth client ID and stable credential scope strings.
- No user aggregate, API, storage, SDK, or runtime code.

## Ownership

- This doc rule `src/credential-core/**`.
- Parent doc: `src/AGENTS.md`.

## Verification

- `cargo check -p credential-core --all-targets --all-features`
- `cargo test -p credential-core --all-features`
