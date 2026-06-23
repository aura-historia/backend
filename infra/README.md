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
src/config.ts              # stage configuration
src/parameters.ts          # deployment parameters/artifact version inputs
src/constructs/            # focused infrastructure modules
  api.ts                   # HTTP API Gateway routes and JWT authorizer
  cognito.ts               # Cognito user pool, clients, hosted UI domain
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
