# DOX

## Purpose

- Own GitHub automation and workflow files.

## Core Design

- `workflows/` drive integrate, deploy, sonar, and repo automation.
- Deploy workflow deploys split CDK stacks from one stage prefix and merges stack outputs for smoke tests.
- Workflow change can change CI gate, deploy path, or DOX contract for many crates.

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
- Run touched local command when practical.

## Child DOX Index

- None.
