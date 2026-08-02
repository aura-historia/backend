**Think caveman. Talk caveman. Few word.**

---

# DOX

## Purpose

- Own repo DOX rail.
- Own root files: `Cargo.toml`, `Cargo.lock`, `README.md`, `LICENSE`, `.cargo/`, `depgraph-rules.toml`, repo config.

## Core Design

- Repo be Rust workspace for AWS serverless backend.
- `src/` hold crates. `migrations/` hold Postgres business schema. `infra/` shape cloud. `docs/` hold public contract. `mjml/` and `opensearch/` hold shared assets.
- Domain crates keep rules. API and Lambda crates stay thin around transport and runtime glue.

## Ownership

- This file rule whole repo.
- Child doc rule deeper path.
- Near doc win detail. Child no break parent.

## Local Contracts

- Read root, then each `AGENTS.md` on path, before edit.
- Re-read in same session. No trust memory.
- After meaningful change, do DOX pass.
- Update nearest doc when purpose, shape, workflow, contract, input, output, limit, side effect, or user pref change.
- Refresh child index. Kill stale words.
- Put durable user prefs here or nearest child doc.

## Work Guidance

- Think caveman. Talk caveman. Few word.
- Less is more.
- Keep docs short, clear, current.
- In `src`, make doc by crate. No module doc unless module become crate boundary.
- Always refer to `docs/arch.md` for architecture or any design decisions. It also includes detailed guidelines on DDD, persistence, APIs, logging, testing and co.

## Verification

- Rust all: `cargo check --workspace`
- Rust dep graph: `cargo depgraph-check check`
- Rust tests: `cargo test --workspace --lib --all-features`
- Infra test: `npm --prefix infra test`
- Infra synth: `npm --prefix infra run synth:all`

## Child DOX Index

- `.agents/AGENTS.md` — project-local agent skills.
- `.github/AGENTS.md` — GitHub flow.
- `docs/AGENTS.md` — public docs.
- `infra/AGENTS.md` — CDK infra.
- `mjml/AGENTS.md` — email templates.
- `opensearch/AGENTS.md` — shared OpenSearch assets.
- `src/AGENTS.md` — Rust crates and `src/opensearch/`.
