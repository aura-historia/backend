# DOX

## Purpose

- Own CDK app, constructs, and infra tests.
- Own cloud contract for backend crates.

## Core Design

- One stack compose focused constructs: storage, queues, search, lambdas, eventing, API, workflow, identity, observability.
- `src/application-stack.ts` wire big pieces. Keep it orchestration-only.
- `src/config.ts` own stage drift. Same stack shape for `prod`, `dev`, `ephemeral`. Difference must be on purpose.
- Prefer typed definition maps for repeated resources like Lambdas and queues. No copy-paste forests.
- CloudFormation input surface stay tiny. Deploy version come from `CommitSHA`. Secrets and external IDs come from SSM dynamic refs. Fixed shared buckets stay fixed.
- Infra own runtime glue: env vars, triggers, schedules, IAM, queue wiring, outputs, retention, alarms. Rust crates own business rules.

## Ownership

- This doc rule `infra/**`.
- Keep app entry, constructs, tests, synth flow, and deploy contract in sync.

## Local Contracts

- Read root, then here, before edit.
- If code adds or changes env var, trigger, queue, event bus, schedule, API route, Cognito need, workflow step, search dependency, or IAM action, update infra in same change.
- If infra change shifts behavior, update nearest code doc, tests, and public docs when contract go public.
- New Lambda means: add definition, wire event source or route, grant IAM, set env, wire deploy flow, and update test wiring when needed.
- Keep outputs intentional. Export stable values people or tests really use. No noisy output spam.

## Work Guidance

- Think caveman. Talk caveman. Few word.
- Keep stage drift low. Dev, prod, ephemeral should differ on purpose only.
- Prefer high-level CDK constructs. Drop lower only when CDK no fit or exact CloudFormation control matter.
- Keep names stable, predictable, and stage-suffixed.
- Least-privilege IAM first. Grant exact action and resource. Wildcard only when AWS force hand.
- Keep secrets out of code and templates when possible. Prefer SSM dynamic refs or deploy-time imports.
- Prod safety first: retain durable prod data, keep rollback simple, avoid surprise replacement of stateful resources.
- No hotswap mindset. Prefer normal CloudFormation change-set rollout and rollback semantics.
- API Lambdas be short request-response handlers. Keep timeout and memory conservative.
- Queue workers do external I/O and side effects. Tune batch, retry, and visibility timeout conservatively. Visibility timeout should clearly exceed Lambda timeout.
- Pipeline workers be explicit about concurrency and long-running cost. For heavy queue workers, visibility timeout should be around `6x` Lambda timeout unless real reason says otherwise.
- Scheduled sync jobs should fail fast, not camp for long timeouts.
- Ephemeral stage should mock or localize third-party integration when possible.
- Keep prod-only alarms and noisy observability out of lower stages unless signal justify cost.

## Verification

- `npm --prefix infra run build`
- `npm --prefix infra test`
- `npm --prefix infra run synth -- --context stage=dev`
- `npm --prefix infra run synth -- --context stage=prod`
- `npm --prefix infra run synth -- --context stage=ephemeral`
- `npm --prefix infra run synth:all`

## Child DOX Index

- None.
