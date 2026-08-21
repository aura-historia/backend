# DOX

## Purpose

- Own GitHub automation and workflow files.

## Core Design

- `workflows/` drive integrate, deploy, and repo automation.
- Integrate workflow checks the exact `common` compatibility baseline against the explicit pull-request base or push predecessor, rejects growth even when an allowlist is edited, runs checker mutation tests, checks Rust dependency graph rules, runs Rust crate tests with required coverage, processes only `coverage-profraw` profiles, and uploads merged LCOV to SonarCloud. Profile search/generation errors, missing coverage input, or an empty report fail CI.
- Deploy workflow deploys split CDK stacks from one stage prefix, pushes active Lambda artifacts, and merges stack outputs for smoke tests.
- Workflow change can change CI gate, deploy path, or DOX contract for many crates.
- Command failure MUST fail its job. `always()` only for cleanup; explicit fallback must fail if recovery fails.

## Ownership

- This doc rule `.github/**`.
- Keep workflow names, triggers, permissions, cache use, and called scripts honest.

## Local Contracts

- Read root, then here, before edit.
- Update this file when workflow shape or automation contract change.
- If workflow starts checking new crate or asset, make sure owning doc say so too.

## Work Guidance

- Think caveman. Talk caveman. Few word.
- CI truth matter. No stale job, path, or secret name.

## Verification

- Read changed workflow end to end.
- Run touched local command when practical, including `cargo depgraph-check check` when graph rules change.

## Child DOX Index

- None.
