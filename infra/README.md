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
# Both application stages and the delivery foundation:
npm run synth:all
```

These commands do not deploy. Deployment-helper tests run from the repository root:

```sh
node --test ci/*.test.cjs ci/delivery/*.test.cjs
```

## Delivery foundation

The manual release entry runs the full candidate test suite, requires protected
`aws-release-build` approval before a new shared build, then requires a separate
`aws-dev` or `aws-prod` approval for the reviewed target deployment. Promotion
reuses immutable artifacts. Routine delivery leaves DMS running.

Synthesize the independent control plane with:

```sh
npm run synth -- --app 'npx ts-node --prefer-ts-exts bin/delivery.ts'
```

Production is enabled by default; `--context devEnabled=true` additionally provisions
a dev deployment pipeline. This flag controls delivery and does not decommission
existing workloads or remove their idle costs.

Bootstrap requires approved control-source key/version, workflow revision, build
configuration digest, GitHub OIDC/environment protections, reviewer model permissions
and target resource identities. Existing repositories and moved resources require an
explicit ownership transition. The final split templates are not an in-place migration
procedure. Follow the [infrastructure runbook](../docs/infra.md) before any AWS write.

The former container-artifact app describes the legacy repository ownership; do not
deploy it alongside the delivery foundation as a competing owner. Preserve existing
repository identities and image digests during the separately approved transition.

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
