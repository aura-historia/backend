# DOX

## Purpose

- Own scheduled process runtime.
- Trigger service use cases. Own no business rule or service port.

## Core Design

- `cron_tab` triggers only. Aura owns overlap, timeout, panic handling, shutdown drain, and status.
- UTC schedules only. Job execution returns `Result` before `cron_tab` boundary. Preserve job error sources.
- Start health before scheduler; mark ready only after scheduler starts. Stop accepting work, stop scheduler, then drain active jobs when shutdown, health, or scheduler fails.
- Observe scheduler termination. SIGINT and SIGTERM start graceful shutdown.
- Runtime wiring composes adapters. No `aura-historia-worker` or `common` dependency.
- `SEARCH_FILTER_PERIODIC_MATCH_CRON` is a validated seven-field UTC expression. `PERIODIC_MATCH_MAX_RUN_SECONDS` must be positive.

## Ownership

- This doc rules `src/aura-historia-cron/**`.
- Parent: `src/AGENTS.md`.

## Work Guidance

- Keep runtime glue thin.
- Never queue an overlapping tick.
- Do not log job payloads, credentials, or secrets.
- Emit `cron.scheduler.started`, `cron.scheduler.drained`, `cron.job.started`, and `cron.job.completed`. Job completion needs `job`, `outcome`, and `duration_ms`.

## Verification

- `cargo check -p aura-historia-cron --all-targets --all-features`
- `cargo test -p aura-historia-cron --all-features`

## Child DOX Index

- None.
