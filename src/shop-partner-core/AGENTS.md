# DOX

## Purpose

- Own `shop-partner-core` crate.
- Own canonical Partner Shop Application domain.

## Core Design

- Domain-only crate.
- Root modules: `partner_shop_application`, `partner_shop_application_id`, `partner_shop_application_state`.
- Owns `PartnerShopApplicationId`; applications use explicit canonical `user-core::UserId` and `shop-core::ShopId` references.
- Applications always link one valid `ShopId`; new applications use a draft shop created before the application.
- Applications support explicit `SUBMITTED → IN_REVIEW → APPROVED|REJECTED` and withdrawal transitions. Terminal states reject later transitions.

## Ownership

- This doc rule `src/shop-partner-core/**`.
- Parent doc: `src/AGENTS.md`.

## Work Guidance

- Think caveman. Talk caveman. Few word.
- No persistence, transport, runtime glue.

## Verification

- `cargo check -p shop-partner-core`
- `cargo test -p shop-partner-core --all-features`

## Child DOX Index

- None.
