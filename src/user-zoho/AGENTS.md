# DOX

## Purpose

- Own Zoho Campaigns newsletter subscription adapter.
- Implement `user-service` newsletter writer port.

## Core Design

- Own Zoho OAuth refresh and token cache.
- Keep Zoho request and response shapes private.
- Never log email, request or response body, token, or credential.

## Ownership

- This doc rule `src/user-zoho/**`.
- Parent doc: `src/AGENTS.md`.

## Work Guidance

- Map known Zoho invalid-email codes to port error.
- Keep provider failures boxed at port boundary.
- Keep wire tests beside implementation.

## Verification

- `cargo test -p user-zoho --all-features`

## Child DOX Index

- None.
