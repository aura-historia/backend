# DOX

## Purpose

- Own CDK infra code and tests.

## Core Design

- Infra synthesize stage-specific AWS stacks for backend crates and shared resources.
- Lambda env, triggers, queues, tables, indexes, and permissions live here as durable cloud contract.

## Ownership

- This doc rule `infra/**`.
- Keep app entry, constructs, tests, and synth flow in sync.

## Local Contracts

- Read root, then here, before edit.
- If crate env var, trigger, timeout, memory, queue, or IAM need change, update infra too.
- Infra change that shifts behavior must update nearest code docs and tests.

## Work Guidance

- Think caveman. Talk caveman. Few word.
- Keep stage drift low. Dev, prod, ephemeral should differ on purpose only.

## Verification

- `npm --prefix infra run build`
- `npm --prefix infra test`
- `npm --prefix infra run synth:all`

## Child DOX Index

- None.
