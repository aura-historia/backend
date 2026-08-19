# DOX

## Purpose

- Own `shop-core` crate.
- Own canonical Shop domain types for migration.

## Core Design

- Domain-only crate.
- Root modules: `address`, `affiliate_configuration`, `continent`, `lifecycle`, `partner_status`, `shop`, `shop_search`, `shop_type`, `sort_shop_field`, `woocommerce_webhook_secret`.
- `shop::Shop` is canonical aggregate. Fields private. Rehydrate boundary public for adapter crates.
- Shop lifecycle defaults to `Drafted`; partner applications may create draft shops. A discarded draft is terminal and cannot be published.
- Uses `domain-primitives` for neutral change outcomes plus pure `money` and `localization` values.
- No dependency on `shop-service`, legacy `shop`, or adapters.

## Ownership

- This doc rule `src/shop-core/**`.
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

- `cargo check -p shop-core`
- `cargo test -p shop-core --all-features`

## Child DOX Index

- None.
