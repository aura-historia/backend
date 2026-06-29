# DOX

## Purpose

- Own `aws-tests` crate.

## Core Design

- Parent test crate for AWS smoke and staging suites.
- Child crates: `aws-tests-common`, `smoking-tests`.
- Main neighbors: `aws-tests-common`, `smoking-tests`.
- Parent crate exists to group child executables or suites and keep their map discoverable.

## Ownership

- This doc rule `src/aws-tests/**`.
- Parent doc: `src/AGENTS.md`.
- Child docs below rule deeper child crates.

## Local Contracts

- Read `AGENTS.md`, `src/AGENTS.md`, then here, before edit.
- New doc only for child crate. No module doc.
- Update this file when crate contract, route/event shape, env vars, or child index change.
- Keep child crate list honest. Shared parent glue stay tiny.

## Work Guidance

- Think caveman. Talk caveman. Few word.
- Parent crate own map and shared glue. Real work live in child crates.

## Verification

- `cargo check -p aws-tests`

## Child DOX Index

- `src/aws-tests/src/aws-tests-common/AGENTS.md` — `aws-tests-common` crate.
- `src/aws-tests/src/smoking-tests/AGENTS.md` — `smoking-tests` crate.
