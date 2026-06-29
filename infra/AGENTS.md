## Purpose

- Own CDK infra code and tests.

## Ownership

- This doc rule `infra/**`.
- It keeps app entry, constructs, tests, and synth flow.

## Local Contracts

- Read root, then here, before edit.
- Update this file when stack shape, synth flow, or test contract change.

## Work Guidance

- Think caveman. Talk caveman. Few word.
- Keep infra code and tests in lockstep.

## Verification

- `npm --prefix infra run build`
- `npm --prefix infra test`
- `npm --prefix infra run synth:all`

## Child DOX Index

- None.
