# DOX

## Purpose

- Own `user-core` crate.
- Own canonical User domain types for migration.

## Core Design

- Domain-only crate.
- Root modules: `access_token`, `first_name`, `last_name`, `name`, `newsletter_subscription`, `role`, `sort_user_field`, `tier`, `user`, `user_search`.
- `user::User` is canonical aggregate. Fields private. Rehydrate boundary public for adapter crates.
- Access-token domain types and canonical scope enum live here; persistence stays behind service ports.
- Access-token aggregate has no storage metadata; repositories/read models own timestamps.
- `newsletter_subscription::NewsletterSubscription` owns newsletter recipient values and optional linked user identity.
- User sort defaults to `Name`; no score sort in canonical user.
- Uses `domain-primitives` only for neutral change outcomes.
- No dependency on `user-service`, legacy `user`, or adapters.

## Ownership

- This doc rule `src/user-core/**`.
- Parent doc: `src/AGENTS.md`.
- No child doc below.

## Local Contracts

- Read `AGENTS.md`, `src/AGENTS.md`, then here, before edit.
- New doc only for child crate. No module doc.
- Update this file when crate contract or dependency edge changes.

## Work Guidance

- Think caveman. Talk caveman. Few word.
- Keep business rules here.
- No persistence, transport, or runtime glue.

## Verification

- `cargo check -p user-core`
- `cargo test -p user-core --all-features`

## Child DOX Index

- None.
