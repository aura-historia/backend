# DOX

## Purpose

- Own `ci-determinator` crate.

## Core Design

- CLI that maps changed files to integration and acceptance test crates, including canonical notification adapters.
- Workspace utility. Inputs be changed file paths; output be stable machine JSON.

## Ownership

- This doc rule `src/ci-determinator/**`.
- Parent doc: `src/AGENTS.md`.
- No child doc below.

## Local Contracts

- Read `AGENTS.md`, `src/AGENTS.md`, then here, before edit.
- New doc only for child crate. No module doc.
- Update this file when crate contract, route/event shape, env vars, or child index change.
- Output JSON shape and path rules be contract. Change with care.

## Work Guidance

- Think caveman. Talk caveman. Few word.
- Keep package path lists and change rules current.

## Verification

- `cargo check -p ci-determinator`
- `cargo test -p ci-determinator --all-features`

## Child DOX Index

- None.
