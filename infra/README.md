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

## Coordinated releases

**Release (CD)** handles routine forward releases with one protected approval, automatic
DMS pause, foundation, private migrations, application deployment and verified capture
recovery. Initial FX, bootstrap, rollback and incompatible cutovers remain dedicated
operations. See [setup and IAM](../docs/infra.md#release-ownership-setup),
[requirement declarations](../docs/infra.md#release-requirements), and
[recovery](../docs/infra.md#release-recovery).

Before adoption, synthesize the independent operational state/monitor stack:

```sh
npm run cdk -- synth aura-historia-release-operations \
  --app 'npx ts-node --prefer-ts-exts bin/releases.ts'
```

Provision it separately under approved setup, configure the bucket/account variables and
protected reviewers, confirm alarm delivery, then run **Adopt release baseline** for the
verified deployed source. Local synth/tests do not perform that setup or deploy a release.

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
