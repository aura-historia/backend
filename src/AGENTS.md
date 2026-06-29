## Purpose

- Own Rust workspace map.
- Own `src/lib.rs` and `src/opensearch/`.
- Point work to crate docs.

## Ownership

- This doc rule `src/**`.
- Crate doc rule its crate path.
- Near doc win detail.

## Local Contracts

- Read `AGENTS.md`, then here, then crate doc, before edit.
- New `src` doc go at crate root. No module doc.
- Update nearest doc when crate map, shared assets, workflow, or child index change.

## Work Guidance

- Think caveman. Talk caveman. Few word.
- Keep change inside one crate when can.
- Shared `src/opensearch/` assets stay here unless they grow own boundary.

## Verification

- Whole workspace: `cargo check --workspace`
- Tight crate check: `cargo check -p <crate>`

## Child DOX Index

- `src/acceptance-tests/AGENTS.md` — `acceptance-tests` crate.
- `src/aws-tests/AGENTS.md` — `aws-tests` crate.
- `src/ci-determinator/AGENTS.md` — `ci-determinator` crate.
- `src/cloudwatch-log-retention-lambda/AGENTS.md` — `cloudwatch-log-retention-lambda` crate.
- `src/cognito/AGENTS.md` — `cognito` crate.
- `src/cognito-post-confirmation/AGENTS.md` — `cognito-post-confirmation` crate.
- `src/common/AGENTS.md` — `common` crate.
- `src/crawler/AGENTS.md` — `crawler` crate.
- `src/fxrate/AGENTS.md` — `fxrate` crate.
- `src/fxrate-lambda/AGENTS.md` — `fxrate-lambda` crate.
- `src/geo/AGENTS.md` — `geo` crate.
- `src/newsletter-api/AGENTS.md` — `newsletter-api` crate.
- `src/notification/AGENTS.md` — `notification` crate.
- `src/notification-api/AGENTS.md` — `notification-api` crate.
- `src/notification-send/AGENTS.md` — `notification-send` crate.
- `src/oauth/AGENTS.md` — `oauth` crate.
- `src/oauth-api/AGENTS.md` — `oauth-api` crate.
- `src/partner-shop-application/AGENTS.md` — `partner-shop-application` crate.
- `src/partner-shop-application-api/AGENTS.md` — `partner-shop-application-api` crate.
- `src/partner-shop-application-lambda/AGENTS.md` — `partner-shop-application-lambda` crate.
- `src/product/AGENTS.md` — `product` crate.
- `src/product-api/AGENTS.md` — `product-api` crate.
- `src/product-api-partner/AGENTS.md` — `product-api-partner` crate.
- `src/product-lambda/AGENTS.md` — `product-lambda` crate.
- `src/product-personalization/AGENTS.md` — `product-personalization` crate.
- `src/product-pipeline/AGENTS.md` — `product-pipeline` crate.
- `src/product-watchlist/AGENTS.md` — `product-watchlist` crate.
- `src/product-watchlist-api/AGENTS.md` — `product-watchlist-api` crate.
- `src/search-filter/AGENTS.md` — `search-filter` crate.
- `src/search-filter-api/AGENTS.md` — `search-filter-api` crate.
- `src/search-filter-lambda/AGENTS.md` — `search-filter-lambda` crate.
- `src/search-filter-periodic-match/AGENTS.md` — `search-filter-periodic-match` crate.
- `src/shop/AGENTS.md` — `shop` crate.
- `src/shop-api/AGENTS.md` — `shop-api` crate.
- `src/shop-lambda/AGENTS.md` — `shop-lambda` crate.
- `src/shopify-lambda/AGENTS.md` — `shopify-lambda` crate.
- `src/stripe-api/AGENTS.md` — `stripe-api` crate.
- `src/stripe-lambda/AGENTS.md` — `stripe-lambda` crate.
- `src/test-api/AGENTS.md` — `test-api` crate.
- `src/user/AGENTS.md` — `user` crate.
- `src/user-api/AGENTS.md` — `user-api` crate.
- `src/user-lambda/AGENTS.md` — `user-lambda` crate.
- `src/webhook-api/AGENTS.md` — `webhook-api` crate.
