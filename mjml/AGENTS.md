# DOX

## Purpose

- Own MJML email templates.

## Core Design

- Templates grouped by feature: partnership application, search filter, watchlist.
- Notification code render these assets. Template names and variables be durable contract. ProductListing notices use `listing_source_name` and Rust-built `product_listing_url`; `first_name` is the greeting variable.
- Notification product `title` may be absent; guard title blocks and preview text.

## Ownership

- This doc rule `mjml/**`.
- Keep template structure and referenced partial data aligned with notification code.

## Local Contracts

- Read root, then here, before edit.
- If template variable or file path change, update caller code and tests.
- Keep copy, locale, and layout differences intentional.

## Work Guidance

- Think caveman. Talk caveman. Few word.
- Email must read clean in real inbox, not just source.

## Verification

- Review rendered markup path in touched notification flow when practical.

## Child DOX Index

- None.
