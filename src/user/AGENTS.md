# DOX

## Purpose

- Own `user` crate.

## Core Design

- User domain, repositories, search projection, and services.
- Root modules: `core`, `data`, `dynamodb`, `opensearch`, `service`.
- New migration contracts live beside old DynamoDB/OpenSearch paths: canonical aggregate in `core::user_aggregate`, use-case traits in `service::use_cases`, capability ports in `service::ports`, bundle in `service::use_case_bundle`.
- Canonical `User` keeps business state private, leaves operational metadata and partner-shop joins to readers, targets Postgres via `UserRepository`, and keeps access tokens behind `AccessTokenStore` for DynamoDB.
- Old `core::user::User` and `service::{command,user_service}` stay until cutover.

- Main neighbors: `common`, `geo`.
- Library crate. Keep domain, persistence, and service seams explicit.

## Ownership

- This doc rule `src/user/**`.
- Parent doc: `src/AGENTS.md`.
- No child doc below.

## Local Contracts

- Read `AGENTS.md`, `src/AGENTS.md`, then here, before edit.
- New doc only for child crate. No module doc.
- Update this file when crate contract, route/event shape, env vars, or child index change.
- Keep business rules here, not leaked into callers.

## Work Guidance

- Think caveman. Talk caveman. Few word.
- Service and repository split stay clean.
- Keep transport and runtime glue out of domain core.

## Verification

- `cargo check -p user`
- `cargo test -p user --all-features`

## Child DOX Index

- None.
