# DOX

## Purpose

- Own GitHub automation and workflow files.

## Core Design

- `workflows/` drive integrate, deploy, initialize, and repo automation.
- Workflows load the pinned Rust compiler and required components from the root `rust-toolchain.toml` through `rustup show`; Dependabot Cargo updates track that file.
- Integrate workflow compiles every discovered MJML source, checks Rust dependency graph rules, runs Rust crate tests with required coverage, processes only `coverage-profraw` profiles, and uploads merged LCOV to SonarCloud. Profile search/generation errors, missing coverage input, or an empty report fail CI. Changes under `migrations/**` trigger integration validation.
- Deploy workflow packages and pushes the eight active Rust Lambda artifacts with locked Cargo/cargo-lambda inputs, publishes all 25 active MJML templates—10 `partnership-application`, 5 `search-filter/match`, and 10 `watchlist/product-update` (availability and price), each in `de`, `en`, `es`, `fr`, and `it`—and tests CDK on pushes. Pushes never deploy. Manual normal deploy takes only stage and a full artifact commit SHA, checks out that exact source for both infrastructure checks and deployment, then deploys initialized network, data, private initialization, compute, API, and prod observability stacks with `CommitSHA`, preserving the consumer mapping value and never starting or resetting DMS. Manual initialize takes only stage and commit SHA; uses the protected per-stage AWS environment; shares the `aws-deploy-<stage>` non-cancelling lock with deploy; always applies network/data from the selected revision before the importing initialization stack; deploys the private migration runtime and normal compute/API with event consumers off; synchronously runs migration then FX; then enables normal eventing with the same `CommitSHA`. It declares but never starts DMS. The separately approved first start supplies the actual source slot/LSN; later recovery uses DMS resume processing. The CI deploy role must allow `lambda:InvokeFunction` on exactly the stage-specific `database-migration-lambda-<stage>` and `fxrate-lambda-<stage>` functions. Changes under `migrations/**` trigger deployment validation.
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
