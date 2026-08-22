# DOX

## Purpose

- Own scheduled process runtime.
- Trigger service use cases. Own no business rule or service port.

## Core Design

- `cron_tab` triggers only. Aura owns overlap, timeout, panic handling, shutdown drain, and status.
- UTC schedules only. Job execution returns `Result` before `cron_tab` boundary.
- Runtime wiring composes adapters. No `aura-historia-worker` or `common` dependency.

## Ownership

- This doc rules `src/aura-historia-cron/**`.
- Parent: `src/AGENTS.md`.

## Work Guidance

- Keep runtime glue thin.
- Never queue an overlapping tick.
- Do not log job payloads, credentials, or secrets.

## Verification

- `cargo check -p aura-historia-cron --all-targets --all-features`
- `cargo test -p aura-historia-cron --all-features`

## Child DOX Index

- None.
