# DOX

## Purpose

- Own EMAIL channel target contract.

## Core Design

- Defines typed EMAIL targets and target lookup port.
- No AWS, SQLx, provider, or runtime code.
- Provider adapters consume this contract. Storage adapters implement it.

## Ownership

- This doc rules `src/notification-email/**`.
- Parent doc: `src/AGENTS.md`.

## Verification

- `cargo check -p notification-email`
- `cargo test -p notification-email --all-features`
