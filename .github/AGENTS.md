# DOX

## Purpose

- Own GitHub automation and workflow files.

## Core Design

- `workflows/` drive integrate, deploy, and repo automation.
- Workflows load the pinned Rust compiler and required components from the root `rust-toolchain.toml` through `rustup show`; Dependabot Cargo updates track that file.
- Integrate workflow checks Rust dependency graph rules, runs Rust crate tests with required coverage, processes only `coverage-profraw` profiles, and uploads merged LCOV to SonarCloud. Profile search/generation errors, missing coverage input, or an empty report fail CI. Changes under `migrations/**` trigger integration validation.
- Deploy workflow deploys split CDK stacks from one stage prefix, pushes active Lambda artifacts, and merges stack outputs for smoke tests. Changes under `migrations/**` trigger deployment validation.
- Workflow change can change CI gate, deploy path, or DOX contract for many crates.
- `workflows/test-images.yml` publishes only trusted-branch immutable test images; the pinned Postgres pg-ttl reference lives in `src/test-api/postgres/image-ref.txt`. Integration jobs use package-read access and `GITHUB_TOKEN` GHCR login to consume private images.
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
