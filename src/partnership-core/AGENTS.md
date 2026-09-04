# DOX

## Purpose

- Own Partnership identity and PartnershipApplication domain state.

## Core Design

- `Partnership` has only ID and Party ID; no lifecycle or status.
- Applications own proposal and `SUBMITTED → IN_REVIEW → APPROVED|REJECTED`, with withdrawal from submitted or review. Approved state owns immutable Partnership and ListingSource result IDs.
- `PartnershipApplicationSearch`, `PartnershipProposalType`, and `SortPartnershipApplicationField` own the admin review-query vocabulary; persisted and query enum codes remain exact canonical identifiers.
- Proposed Party and ListingSource values are intent only. Approval creates durable Party and ListingSource state.
- No service, adapter, transport, or Shop dependency.

## Verification

- `cargo check -p partnership-core`
- `cargo test -p partnership-core --all-features`
