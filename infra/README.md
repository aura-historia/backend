# Aura Historia infrastructure

This directory contains the AWS CDK app for the Aura Historia backend.

The application is split into small CDK/CloudFormation stacks with typed
configuration objects instead of hand-written templates. The same stack code is
synthesized for:

- `prod` — real AWS production resources, production alarms enabled
- `dev` — real AWS development resources, production alarms disabled
- `ephemeral` — LocalStack resources, including a local OpenSearch domain

## Structure

```text
bin/app.ts                 # CDK entrypoint and stage selection
src/application-stack.ts   # data, compute, API, observability stack composition
src/config.ts              # stage configuration, fixed buckets, SSM dynamic refs
src/parameters.ts          # deployment artifact version input
src/resources/             # synth-time resources, e.g. Cognito email HTML and inline JS
src/constructs/            # focused infrastructure modules
  api.ts                   # HTTP API Gateway routes, domain, CloudFront, WAF, CORS, JWT authorizer
  cognito.ts               # Cognito user pool, public client, IdPs, hosted UI domain
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

Synth creates these stacks per stage:

- `application-{stage}-data` — DynamoDB, SQS, and LocalStack OpenSearch
- `application-{stage}-compute` — Lambdas, Cognito, eventing, schedules
- `application-{stage}-api` — HTTP API Gateway routes, domain, CloudFront, integrations, authorizer
- `application-prod-observability` — prod-only alarms and alarm topic

Dev CloudFront owns the wildcard alias `*.dev.aura-historia.com`; the API URL stays
`api.dev.aura-historia.com`. This avoids stale exact DNS targets blocking distribution
creation. Prod uses the exact alias `api.aura-historia.com`.

Deployments should use `cdk deploy --all` without hotswap. CI uses
CloudFormation change sets (`--method change-set`) so stack updates keep
CloudFormation's normal rollback semantics.

Deployments do not require a full CDK bootstrap stack in the target account/region.
Each stack uses `CliCredentialsStackSynthesizer` with the existing staging bucket
`aura-historia-cfn-artifcats-eu-central-1`. CDK uploads large CloudFormation
templates and any future file assets under the stage prefix (`${stage}/`). Lambda
ZIPs and scheduled Fargate images are still referenced as prebuilt S3/ECR
artifacts keyed by `CommitSHA`, not as CDK-managed assets.

Rollback is performed by redeploying a previous `CommitSHA` parameter value to the
compute stack. Lambda ZIP keys and mail-template prefixes include that SHA, so CDK
points compute resources back to the previously uploaded artifacts.

## Deployment inputs

Only the compute stack exposes a CloudFormation parameter:

- `CommitSHA` — artifact version to deploy or roll back to

The Lambda artifact and mail-template buckets are fixed in `src/config.ts`:

- `aura-historia-binary-artifacts-eu-central-1`
- `aura-historia-mail-templates-eu-central-1`

LocalStack acceptance tests synthesize one ephemeral stack with CDK context
`singleStack=true` and pass the host-mapped edge port as `localStackMappedPort`.
These values are synth-time context, not CloudFormation parameters.

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
/certificates/{stage}/api-regional-certificate-arn
/certificates/{stage}/api-cloudfront-certificate-arn
/secrets/{stage}/gemini-api-key
/secrets/{stage}/google-application-credentials
/secrets/{stage}/google-geocoding-api-key

/secrets/{stage}/zoho-accounts-url
/secrets/{stage}/zoho-campaigns-url
/secrets/{stage}/zoho-client-id
/secrets/{stage}/zoho-client-secret
/secrets/{stage}/zoho-list-key
/secrets/{stage}/zoho-refresh-token
```

`fxrate-lambda` currently reads `/fxratesapi/prod/api-token` for the scheduled
sync. On first real-stage compute-stack creation, a custom resource synchronously
invokes this same Lambda with stable deployment source ID `deployment:fxrate:initial:{stage}:v1`.
Deployment fails when this initial capture fails; it must run after PostgreSQL
business migrations. Updates, deletes, and `ephemeral` do not invoke it. The
`ephemeral` stage uses local/mock values for third-party integrations where possible.
