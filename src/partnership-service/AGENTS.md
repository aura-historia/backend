# DOX

## Purpose

- Own Partnership application and source-grant use cases.

## Core Design

- Submit stores intent only; it creates no Party or ListingSource.
- Approval owns one PostgreSQL UoW: resolve/create Party and ListingSource, find-or-create Partnership, grant membership and source access, update application, create the applicant notification through `NotificationCreatorFactory`, then commit.
- Application reads are reader-owned. Source authorization is a focused service port.
- Rejection resolves its immutable Party/ListingSource snapshot, updates application, and creates its applicant notification in the same PostgreSQL UoW. Legacy Shop partner flow is not a dependency and remains isolated.

## Verification

- `cargo check -p partnership-service`
- `cargo test -p partnership-service --all-features`
