# Aura Historia infrastructure

This directory contains the AWS CDK app for the Aura Historia backend.

The application stack is described with CDK constructs and small typed configuration
objects instead of hand-written CloudFormation templates. The same stack code is
synthesized for:

- `prod` — real AWS production resources, production alarms enabled
- `dev` — real AWS development resources, production alarms disabled
- `ephemeral` — LocalStack acceptance-test resources, including a local OpenSearch domain

## Structure

```text
bin/app.ts                 # CDK entrypoint and stage selection
src/application-stack.ts   # stack composition and public outputs
src/config.ts              # stage configuration, fixed buckets, SSM dynamic refs
src/parameters.ts          # deployment artifact version input
src/templates/             # synth-time templates, e.g. Cognito verification email HTML
src/constructs/            # focused infrastructure modules
  api.ts                   # HTTP API Gateway routes, CORS, JWT authorizer
  cognito.ts               # Cognito user pool, public client, hosted UI domain
  eventing.ts              # EventBridge buses/rules, SQS mappings, Pipes
  lambdas.ts               # Lambda definitions, env vars, IAM grants
  observability.ts         # prod-only alarms and alarm topic
  opensearch.ts            # external dev/prod endpoint or LocalStack domain
  queues.ts                # SQS queues and DLQs
  storage.ts               # DynamoDB table and indexes
  workflow.ts              # partner application Step Functions workflow
```

## Common commands

```bash
npm ci
npm run build
npm test
npm run synth -- --context stage=dev
npm run synth -- --context stage=prod
npm run synth -- --context stage=ephemeral
```

Deployments should use `cdk deploy` without hotswap. CI uses CloudFormation
change sets (`--method change-set`) so stack updates keep CloudFormation's normal
all-or-nothing rollback semantics.

Rollback is performed by redeploying a previous `CommitSHA` parameter value. Lambda
ZIP keys and mail-template prefixes include that SHA, so CDK points the stack back
to the previously uploaded artifacts.

## Deployment inputs

The synthesized stack intentionally exposes only one CloudFormation parameter:

- `CommitSHA` — artifact version to deploy or roll back to

The Lambda artifact and mail-template buckets are fixed in `src/config.ts`:

- `aura-historia-binary-artifacts-eu-central-1`
- `aura-historia-mail-templates-eu-central-1`

LocalStack acceptance tests pass the host-mapped edge port as CDK context
(`localStackMappedPort`) at synth time, not as a CloudFormation parameter.

## Stage-specific SSM parameters

Real AWS stages resolve external integration settings via CloudFormation dynamic
references to SSM Parameter Store. Required paths are stage-specific for `prod`
and `dev`:

```text
/opensearch/{stage}/endpoint-url
/opensearch/{stage}/username
/opensearch/{stage}/password
/eventbridge/{stage}/stripe-event-bus-name
/eventbridge/{stage}/shopify-event-bus-name
/stripe/{stage}/pro-product-id
/stripe/{stage}/ultimate-product-id
/stripe/{stage}/pro-monthly-price-id
/stripe/{stage}/pro-yearly-price-id
/stripe/{stage}/ultimate-monthly-price-id
/stripe/{stage}/ultimate-yearly-price-id
/secrets/{stage}/gemini-api-key
/secrets/{stage}/google-application-credentials
/secrets/{stage}/google-geocoding-api-key
/secrets/{stage}/stripe-api-key
/secrets/{stage}/zoho-accounts-url
/secrets/{stage}/zoho-campaigns-url
/secrets/{stage}/zoho-client-id
/secrets/{stage}/zoho-client-secret
/secrets/{stage}/zoho-list-key
/secrets/{stage}/zoho-refresh-token
```

`fxrate-lambda` currently reads `/fxratesapi/prod/api-token` for the scheduled
sync. The `ephemeral` stage uses local/mock values for third-party integrations
where possible.
