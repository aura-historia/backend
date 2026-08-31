# DOX

## Purpose

- Own shared fail-closed public HTTP destination IP policy.

## Core Design

- Only classify IP addresses.
- Callers own DNS resolution, URL syntax, redirects, and pinning.
- IPv6 allows global-unicast `2000::/3` after special-use exclusion.

## Ownership

- This doc rule `src/public-network-policy/**`.
- Parent doc: `src/AGENTS.md`.

## Verification

- `cargo test -p public-network-policy`
