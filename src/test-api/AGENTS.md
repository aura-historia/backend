# DOX

## Purpose

- Own `test-api` crate.

## Core Design

- LocalStack and AWS integration test harness.
- Root modules: `api_gateway`, `cloudformation`, `cognito`, `dynamodb`, `eventbridge`, `localstack`, `opensearch`, `rds`, `s3`, `ses`, `signal`, `sqs`.
- Child crates: `test-api-macros`.
- Main neighbors: `aws-tests-common`, `common`, `test-api-macros`, `user`.
- Test crate. Favor stable helpers and black-box assertions.

## Ownership

- This doc rule `src/test-api/**`.
- Parent doc: `src/AGENTS.md`.
- Child docs below rule deeper child crates.

## Local Contracts

- Read `AGENTS.md`, `src/AGENTS.md`, then here, before edit.
- New doc only for child crate. No module doc.
- Update this file when crate contract, route/event shape, env vars, or child index change.
- Keep fixtures deterministic. Add or move suite paths in `src/ci-determinator` when CI scope change.

## Work Guidance

- Think caveman. Talk caveman. Few word.
- Tests prove behavior, not implementation trivia.
- Share helpers before copy-paste fixtures.

## Verification

- `cargo check -p test-api`
- `cargo test -p test-api --all-features`

## Child DOX Index

- `src/test-api/src/test-api-macros/AGENTS.md` — `test-api-macros` crate.
