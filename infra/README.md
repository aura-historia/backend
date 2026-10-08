# Aura Historia infrastructure

AWS CDK tooling for the Rust serverless backend. Application stages are `dev`
and `prod`; the workload region is `eu-central-1`.

For deployment, operator prerequisites, migrations and rollback, start with the
[infrastructure runbook](../docs/infra.md). Source and synth describe intended
configuration, not verified AWS state.

## Local checks

Use Node **26**, matching CI, and the committed npm lockfile. From `infra/`:

```sh
npm ci
npm run build
npm test
npm run synth -- --context stage=dev
npm run synth -- --context stage=prod
# Both application stages:
npm run synth:all
```

These commands do not deploy. Deployment-helper tests run from the repository root:

```sh
node --test ci/*.test.cjs
```

## Container repository setup

See [provisioning and ownership](../docs/infra.md#container-repository-setup).
From `infra/`, synthesize the separate setup stack:

```sh
npm run cdk -- synth aura-historia-container-artifacts \
  --app 'npx ts-node --prefer-ts-exts bin/artifacts.ts'
```

## Editing infrastructure assets

`bin/app.ts` selects the stage; `src/application-stack.ts` composes stacks;
`src/config.ts` and `src/constructs/` own their configuration. Tests live in `test/`.

Cognito verification HTML is generated from `mjml/cognito/verification/en.mjml`.
Do not edit the generated HTML. From the repository root:

```sh
npm --prefix mjml ci
npm --prefix mjml run generate:cognito-verification
```

CI checks that the committed generated asset is fresh. See the
[HTTP front door](../docs/http-api-front-door.md),
[OpenSearch stage runbook](../docs/opensearch-stage.md), and
[worker operations](../docs/durable-worker-runbook.md) for their specific safety gates.
