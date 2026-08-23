# DOX

## Purpose

- Own `shop-core` crate.
- Own canonical Shop domain types.

## Core Design

- Domain-only crate.
- Root modules: `address`, `affiliate_configuration`, `continent`, `domain`, `lifecycle`, `partner_status`, `seller_slug_id`, `shop`, `shop_id`, `shop_name`, `shop_slug_id`, `shop_type`, `sort_shop_field`, `woocommerce_webhook_secret`.
- Owns `Domain`, `ShopId`, `ShopName`, `ShopSlugId`, and `SellerSlugId`. `ShopSearch` is a service query contract.
- `shop::Shop` is canonical aggregate. Fields private. Rehydrate boundary public for adapter crates.
- Shop lifecycle defaults to `Drafted`; partner applications may create draft shops. A discarded draft is terminal and cannot be published.
- Uses `domain-primitives` for neutral change outcomes plus pure `geo`, `money`, and `localization` values.
- No dependency on `shop-service` or adapters.

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
