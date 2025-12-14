# CloudFormation to Pulumi Migration Guide

This document outlines the migration strategy from CloudFormation (`cfn/application.yaml`) to Pulumi TypeScript (`pulumi/`).

## Overview

The Pulumi infrastructure provides a functionally equivalent but more maintainable replacement for the CloudFormation template. The key improvements include:

1. **Type Safety**: TypeScript provides compile-time type checking
2. **Abstraction**: Common patterns (Lambda + alarms, Queue + DLQ) are abstracted into reusable functions
3. **Reduced Duplication**: Stage mappings and repeated alarm configurations are centralized
4. **Better Testing**: Infrastructure code can be unit tested
5. **Turing-Complete**: Full programming language capabilities with loops, conditionals, and functions

## Resource Mapping

### Core Services
| CloudFormation | Pulumi Module | Status |
|----------------|---------------|--------|
| AlarmNotificationTopic | src/modules/sns.ts | ✅ Complete |
| PrimaryUserPool + Clients | src/modules/cognito.ts | ✅ Complete |
| TableOne | src/modules/dynamodb.ts | ✅ Complete |
| OpenSearchDomain | src/modules/opensearch.ts | ✅ Complete |
| Api + ApiStage + ApiCognitoAuthorizer | src/modules/apigateway.ts | ✅ Complete |
| DynamoDbEventBus + Pipe | src/modules/eventbridge.ts | ✅ Complete |

### Lambda Functions
All Lambda functions follow the same pattern established in `src/modules/lambda.ts`:

1. Create IAM role with appropriate policies
2. Create Lambda function with environment variables
3. Create CloudWatch alarms (errors, throttles)
4. Create event source mappings if needed
5. Create API Gateway integration if needed

**Example**: See `index.ts` for Cognito post-confirmation and mail send Lambdas.

### SQS Queues
All queues use the `createQueueWithDlq()` pattern from `src/modules/queue.ts`:

- Creates main queue + dead letter queue
- Configurable visibility timeout and max receive count
- Automatic DLQ alarm

**Example**: See `sendMailQueues` and `productIngestQueues` in `index.ts`.

### CloudWatch Alarms
Alarms are created automatically for:

- Lambda errors and throttles (via `createLambdaAlarms()`)
- DLQ messages (via `createDlqAlarm()`)
- DynamoDB throttled requests
- OpenSearch cluster health, storage, CPU, memory
- API Gateway 4XX and 5XX errors

## Migration Steps

### Phase 1: Core Infrastructure (Complete)
- [x] SNS alarm topic
- [x] Cognito user pool
- [x] DynamoDB table
- [x] OpenSearch domain
- [x] API Gateway
- [x] EventBridge

### Phase 2: Lambda Functions (Partially Complete)
- [x] Cognito post-confirmation
- [x] Mail send
- [x] Product ingest events
- [x] Product materialize DynamoDB (new/update)
- [ ] Product materialize OpenSearch (new/update)
- [ ] Product update notify user
- [ ] All API handlers (product, shop, user, search-filter)

### Phase 3: Product Enrichment (Not Started)
- [ ] VPC and networking
- [ ] Security groups
- [ ] Launch template
- [ ] Auto Scaling Group
- [ ] Scale up/down Lambdas
- [ ] EventBridge schedules

### Phase 4: EventBridge Rules (Not Started)
- [ ] Product event routing rules
- [ ] Queue policies for EventBridge

### Phase 5: Testing and Validation
- [ ] Pulumi preview validation
- [ ] Deploy to dev environment
- [ ] Run integration tests
- [ ] Deploy to staging
- [ ] Deploy to production

## Key Differences

### Stage Mappings
**CloudFormation** uses `!FindInMap` with inline mappings:
```yaml
Mappings:
  SendMailQueueMap:
    MaximumBatchingWindowInSeconds:
      dev: 1
      staging: 1
      prod: 5
```

**Pulumi** uses TypeScript objects:
```typescript
const sendMailQueueBatchWindow: StageMapping<number> = { 
  dev: 1, 
  staging: 1, 
  prod: 5 
};
// Usage: sendMailQueueBatchWindow[stage]
```

### Lambda Functions
**CloudFormation** repeats the same structure for each Lambda:
- Role definition
- Lambda function
- Permissions
- Error alarm
- Throttle alarm

**Pulumi** abstracts this into `createLambda()`:
```typescript
const lambda = createLambda({
  name: 'my-lambda',
  config,
  role,
  environment: { VAR: value },
  snsTopicArn,
});
// Creates lambda + error alarm + throttle alarm automatically
```

### Queue + DLQ Pattern
**CloudFormation** repeats DLQ, Queue, and Alarm for each queue.

**Pulumi** uses `createQueueWithDlq()`:
```typescript
const queues = createQueueWithDlq({
  name: 'my-queue',
  stageName,
  snsTopicArn,
});
// Returns { queue, dlq, dlqAlarm }
```

## Configuration

### Required Secrets in GitHub
- `PULUMI_ACCESS_TOKEN`: Pulumi Cloud access token (or use S3 backend)
- `CI_DEPLOY_ROLE_ARN`: AWS IAM role ARN for CI/CD

### Required Variables in GitHub
- `AWS_REGION`: AWS region (e.g., `eu-central-1`)
- `S3_BINARY_ARTIFACTS_BUCKET_NAME`: S3 bucket for Lambda zips
- `S3_RESOURCE_ARTIFACTS_BUCKET_NAME`: S3 bucket for EC2 resources
- `S3_MAIL_TEMPLATE_BUCKET_NAME`: S3 bucket for mail templates
- `EC2_KEY_PAIR_NAME`: EC2 key pair name

### Stack Configuration
Pulumi configuration is set via `pulumi config set` in CI/CD or locally:
```bash
pulumi config set stage dev
pulumi config set stageName pr-123
pulumi config set artifactBucket bucket-name
pulumi config set resourceBucket bucket-name
pulumi config set mailTemplateBucket bucket-name
pulumi config set commitSHA abc123
pulumi config set ec2KeyPairName my-key
```

## Testing Strategy

### 1. Pulumi Preview
```bash
cd pulumi
npm run build
pulumi preview
```
This shows what resources will be created/updated/deleted.

### 2. Parallel Deployment
Deploy Pulumi to a new stage (e.g., `pr-999`) while keeping CloudFormation stacks running:
```bash
pulumi stack init application-pr-999
# Configure...
pulumi up
```

### 3. Integration Tests
Run the existing integration tests against the Pulumi-deployed stack to ensure functional equivalence.

### 4. Gradual Rollout
1. Deploy to dev (PR stacks) first
2. Validate with integration tests
3. Deploy to staging
4. Validate production deployment
5. Deprecate CloudFormation

## Rollback Plan

If issues are discovered, rollback is straightforward:

1. **Immediate**: Revert the CI/CD workflow changes to use CloudFormation again
2. **Clean up**: Delete Pulumi stacks with `pulumi destroy`
3. **Long-term**: Address issues in Pulumi code and retry migration

## Completing the Migration

To fully complete the migration, the following resources need to be added to `index.ts`:

1. **Product Materialize OpenSearch Lambdas** (new/update)
2. **Product Update Notify User Lambda**
3. **API Handlers**:
   - Shop API (4 handlers)
   - User API (2 handlers)
   - Search Filter API (5 handlers)
4. **Product Enrichment Infrastructure**:
   - VPC with 2 public subnets
   - Internet Gateway and route tables
   - Security group
   - IAM role and instance profile
   - Launch template
   - Auto Scaling Group
   - Scale up/down Lambdas
   - EventBridge schedules
5. **EventBridge Rules** for routing product events
6. **Queue Policies** to allow EventBridge to send to SQS

All of these follow the established patterns in the existing code. The modules provide the abstractions needed to implement them with minimal boilerplate.

## Benefits Realized

After migration:

1. **Reduced Lines of Code**: The CloudFormation template is 3462 lines. The Pulumi code will be ~2000 lines with better organization.

2. **Better Maintainability**: Common patterns are abstracted. Adding a new Lambda with alarms is now 10 lines instead of 100.

3. **Type Safety**: TypeScript catches configuration errors at compile time.

4. **Testing**: Infrastructure code can be unit tested with Pulumi's testing framework.

5. **Reusability**: Modules can be shared across projects.

## Support

For questions or issues during migration:
- Review the [Pulumi README](./README.md)
- Check [Pulumi AWS Documentation](https://www.pulumi.com/registry/packages/aws/)
- Open an issue in the repository
