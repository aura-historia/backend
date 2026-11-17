# Aura Historia infrastructure

AWS CDK app for what stays on AWS while the application runs on the self-hosted host:
CloudFront and its Web ACLs, Cognito with its triggers, the SES configuration set, the
backup bucket and the host's IAM user. It declares `prod` in `eu-central-1` only.

For stack ownership, the origin lock and deployment order, start with the
[infrastructure runbook](../docs/infra.md#infrastructure-boundary). Source and synth
describe intended configuration, not verified AWS state.

## Local checks

Use Node **26**, matching CI, and the committed npm lockfile. From `infra/`:

```sh
npm ci
npm run build
npm test
npm run synth:all
```

These commands do not deploy.

## Editing infrastructure assets

`bin/app.ts` selects the stage; `src/application-stack.ts` composes the `host`, `identity`
and `api` stacks; `src/config.ts` and `src/constructs/` own their configuration. Tests live
in `test/`.

Cognito verification HTML is generated from `mjml/cognito/verification/en.mjml`.
Do not edit the generated HTML. From the repository root:

```sh
npm --prefix mjml ci
npm --prefix mjml run generate:cognito-verification
```

CI checks that the committed generated asset is fresh. See the
[HTTP front door](../docs/http-api-front-door.md) for edge routing, caching and Web ACL rules.
