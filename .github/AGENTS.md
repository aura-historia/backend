# DOX

## Purpose

- Own GitHub automation and workflow files.

## Core Design

- `workflows/` drive integrate, deploy, and repo automation.
- Workflows load the pinned Rust compiler and required components from the root `rust-toolchain.toml` through `rustup show`; Dependabot Cargo updates track that file.
- Integrate workflow compiles every discovered MJML source, checks Rust dependency graph rules, runs Rust crate tests with required coverage, processes only `coverage-profraw` profiles, and uploads merged LCOV to SonarCloud. Profile search/generation errors, missing coverage input, or an empty report fail CI. `.github/scripts/**`, `.cargo/**`, `deploy/**`, `opensearch/**`, and `migrations/**` trigger integration validation on both push and pull request events.
- Every Cargo job binds `COMMIT_SHA` to the SHA of its actual checkout. Rust matrix jobs use package-read GHCR login, explicitly prepare the checked-in Postgres fixture reference, and pass the inspected local image ID to the pull-free fixture. `deployment-checks` installs the reviewed Docker Compose 5.4.0 Linux x86-64 plugin after checksum verification, proves selection through a cleared-environment private Docker config, then runs the actual Compose-config check with one documented cached-helper skip. The cron matrix child removes `AURA_TEST_POSTGRES_IMAGE` and sets `AURA_CRON_ISOLATED_LOCAL_POSTGRES=1`; other crates retain the inspected image ID.
- `workflows/deploy.yml` is a manual inert notice that refuses the legacy CD path. It has no automatic trigger or deployment credentials; this source guard does not disable copies already present on other refs or in-flight runs.
- Workflow change can change CI gate, deploy path, or DOX contract for many crates.
- `workflows/test-images.yml` remains the trusted-branch test-image publisher; the pinned Postgres pg-ttl reference lives in `src/test-api/postgres/image-ref.txt`. Integration jobs use package-read access and `GITHUB_TOKEN` GHCR login to consume private images.
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
