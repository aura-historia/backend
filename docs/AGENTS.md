# DOX

## Purpose

- Own public docs, changelog, OpenAPI, and static site files.

## Core Design

- `swagger.yaml` be public REST contract.
- `CHANGELOG.md` tell API change by pull request.
- `storage.md` owns storage migration and repository conventions.
- `migration-f1-inventory.md` owns checked-in migration baseline, survivor inventory, owner decisions, and cutover gates; it does not prove live deployment.
- Child doc can own deeper subsystem docs when folder become durable boundary.

## Ownership

- This doc rule `docs/**`.
- Keep public docs aligned with shipped behavior. No wish-doc.
- Use `ListingSource`, `Partnership`, and `PartnershipApplication` for active contracts. Legacy names belong only in dated `CHANGELOG.md` entries.

## Local Contracts

- Read root, then here, before edit.
- If endpoint, payload, auth, error, or behavior change, update `docs/swagger.yaml` and `docs/CHANGELOG.md`.
- Update design docs when durable event flow, storage contract, or operator workflow change.
- Keep child doc index fresh.

## Work Guidance

- Think caveman. Talk caveman. Few word.
- Kill stale docs fast.
- Public contract first. Example after.

## Verification

- Diff API docs against code paths you changed.
- Check changed YAML or markdown render for obvious break.

## Child DOX Index

- `arch.md` — workspace architecture rules.
- `auction.md` — Auction domain contract and current implementation status.
- `events/flow.md` — durable event and scheduled-flow contracts.
- `migration-f1-inventory.md` — #1775 migration baseline, contracts, surviving resources, owners, and gates.
- `object-ids.md` — prefixed UUIDv7 object-ID registry and boundary contract.
- `party-and-listing-source.md` — Party, ListingSource, and Partnership contract.
- `product-listing.md` — canonical ProductListing domain contract.
- `storage.md` — canonical storage contracts.
- `swagger.yaml` — public REST contract.
- `CHANGELOG.md` — API change history.
