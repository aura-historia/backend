# DOX

## Purpose

- Own shared OpenSearch protocol mechanics and concrete client TLS policy proven across canonical consumers.

## Core Design

- Own generic wire envelopes and fully-read response helper: search response metadata, hits, timeout error, and status/body/source preservation.
- `tls::OpenSearchTlsConfig` is the approved narrow shared TLS owner for API/worker/cron SDK clients and direct OpenSearch HTTP. No environment reads or generalized configuration framework.
- Never own bounded-context documents, queries, mappings, or adapter behavior.
- Public protocol types are for production adapter boundaries; document type `T` stays adapter-owned.

## Ownership

- This doc rules `src/platform-opensearch/**`.
- Parent doc: `src/AGENTS.md`.

## Work Guidance

- Keep crate narrow. Add protocol shape only after more than one canonical consumer proves need.
- Do not depend on application, bounded-context, adapter, runtime, or transport crates.

## TLS Contract

- `from_inputs(stage, &Url, Option<&str>)` validates exact `dev`/`prod`/`local`/`test`/`ephemeral`. Explicit `local`/`test`/`ephemeral` permit HTTP or absent CA; `dev`/`prod` require HTTPS and CA path. HTTPS still verifies certificates and hostnames in every stage, even without a supplied CA.
- Endpoint needs host, no userinfo/query/fragment. HTTP plus any CA input fails. Present empty paths, unreadable/empty/malformed files fail.
- CA files must be regular, at most 1 MiB. Unix nonblocking open then descriptor metadata rejects FIFO without hanging; symlinks to regular mounted secrets work. Non-Unix file loading fails closed.
- Strict certificate-only PEM bundles; reject keys/garbage/zero certificates, validate DER via pinned rustls roots. Normalize delimiter whitespace so SDK cannot silently omit rotation CAs. Load once, freeze bytes; file changes affect only new configs.
- Config is Clone with redacted Debug. Typed errors contain no input, paths, cert bytes, or raw source chains.
- `configure_transport(&self, TransportBuilder) -> Result<TransportBuilder, OpenSearchTlsError>` reparses frozen validated PEM because SDK Certificate is not Clone. Uses SDK `CertificateValidation::Full` and `disable_proxy`.
- `configure_http(&self, reqwest::ClientBuilder) -> reqwest::ClientBuilder` adds every CA, enables certificate/hostname verification and built-in roots, disables proxies and redirects. No timeout changes.
- Apply to fresh builders using the validated endpoint; do not override security settings afterwards. Trust is additive, not exclusive pinning. SDK 2.4 / internal reqwest 0.13.4 retains default redirect behavior: no exposed SDK customization, no fork. Direct client uses workspace reqwest 0.12.28.
- Runtime owners supply stage/URL/path, build once, share clients, and map later builder/request errors safely. Rotate CA by replacing bundle and restarting/reconstructing clients; remove this helper only after migrating every runtime client to equivalent verification/proxy policy.

## Verification

- `cargo check -p platform-opensearch --all-targets --all-features`
- `cargo test -p platform-opensearch --offline --locked --lib` (bound 240s; integrator updates root lock).
- Beside-code tests use cached in-process fixture, real rustls loopback TLS through both actual clients, and locally generated OpenSSL certificates. Need `openssl`, `timeout`, and Unix `mkfifo`; no network beyond loopback, downloads, cloud, or Docker.
- Keys generated under crate `target/tls-tests/` in private directories, loaded then removed. Commands bounded 10s; client/server I/O bounded 3s. Loopback threads join on finish or drop, including early failures; drop cleanup failures emit a fixed safe warning. Tests cover CA rotation, wrong CA/hostname, missing trust, invalid input, frozen bytes, proxy disablement, and direct no-redirect.

## Child DOX Index

- None.
