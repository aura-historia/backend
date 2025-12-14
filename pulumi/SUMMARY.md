# Pulumi Migration Summary

This document summarizes the CloudFormation to Pulumi migration work.

## What Was Done

### 1. Pulumi Project Setup ✅
Created a complete Pulumi TypeScript project in the `pulumi/` directory with:
- **package.json**: Dependencies for Pulumi and AWS provider
- **tsconfig.json**: TypeScript configuration
- **Pulumi.yaml**: Pulumi project definition
- **eslint.config.mjs**: ESLint configuration for code quality
- **.prettierrc**: Prettier configuration for consistent formatting
- **.gitignore**: Ignoring node_modules, bin, logs
- **.pulumi-ignore**: Ignoring files from Pulumi state

### 2. Reusable Infrastructure Modules ✅
Created best-practice abstraction modules in `src/modules/`:

- **alarms.ts**: CloudWatch alarm patterns
  - `createLambdaAlarms()`: Automatically creates error & throttle alarms for Lambdas
  - `createDlqAlarm()`: Creates DLQ message visibility alarms

- **apigateway.ts**: API Gateway with CORS and JWT authorization
  - `createApiGateway()`: Creates HTTP API with CORS, stage, authorizer, and 4XX/5XX alarms
  - `createApiRoute()`: Integrates Lambda with API Gateway route

- **cognito.ts**: User pool and clients
  - `createUserPool()`: Creates user pool with password policy, MFA, and clients

- **dynamodb.ts**: DynamoDB tables with alarms
  - `createTable()`: Creates table with LSI, GSI, streams, and throttle alarms

- **eventbridge.ts**: EventBridge buses and pipes
  - `createDynamoDBEventBridge()`: Creates event bus and DynamoDB stream pipe
  - `createEventRule()`: Creates event rule with target

- **iam.ts**: IAM role and policy factories
  - `createLambdaRole()`: Creates Lambda execution role
  - Policy helpers: `dynamoDBReadPolicy`, `dynamoDBWritePolicy`, `sqsPollerPolicy`, etc.

- **lambda.ts**: Lambda function patterns
  - `createLambda()`: Creates Lambda with automatic S3 code sourcing, alarms, and standard config
  - `createSqsEventSourceMapping()`: Connects Lambda to SQS queue

- **opensearch.ts**: OpenSearch domain with comprehensive alarms
  - `createOpenSearchDomain()`: Creates domain with cluster health, storage, CPU, and memory alarms

- **queue.ts**: SQS queue + DLQ pattern
  - `createQueueWithDlq()`: Creates main queue, DLQ, and DLQ alarm

- **sns.ts**: SNS topics
  - `createAlarmTopic()`: Creates SNS topic for CloudWatch alarms

### 3. Main Infrastructure Program ✅
Created `index.ts` with:
- Configuration management from Pulumi stack config
- Stage-specific mappings (batch windows, etc.)
- Core services setup (SNS, Cognito, DynamoDB, OpenSearch, API Gateway)
- Example Lambda functions (Cognito post-confirmation, mail send, product ingest)
- Example queue patterns (mail, product ingest, materialize)
- Stack outputs for integration with testing

### 4. CI/CD Integration ✅
Updated `.github/workflows/cicd.yml`:
- Replaced `cfn-lint` job with `pulumi-preview`
- Replaced `aws-cfn-deploy` job with `aws-pulumi-deploy`
- Updated all test jobs to use Pulumi stack outputs instead of CloudFormation outputs
- Added Pulumi CLI installation and configuration steps

Updated `.github/workflows/delete_pr_cfn_stack.yml`:
- Renamed to reflect Pulumi usage (though filename kept for compatibility)
- Replaced CloudFormation deletion with `pulumi destroy`
- Added Pulumi stack removal

### 5. Documentation ✅
Created comprehensive documentation:

- **README.md**: Complete guide to the Pulumi setup
  - Project structure
  - Key abstractions
  - Configuration
  - Deployment instructions
  - Troubleshooting
  - Future improvements

- **MIGRATION.md**: Migration strategy guide
  - Resource mapping from CloudFormation to Pulumi
  - Migration phases
  - Key differences in approach
  - Testing strategy
  - Rollback plan
  - Completion checklist

- **types.ts**: TypeScript types and interfaces
  - `StackConfig`: Configuration interface
  - `StageMapping<T>`: Stage-specific value mapping
  - `AlarmConfig`: Alarm configuration interface

### 6. Code Quality Tools ✅
- ESLint configuration for TypeScript
- Prettier configuration for consistent code style
- Updated .gitignore for Pulumi artifacts

## What Remains

### Immediate Work Needed
The foundation is complete, but the following resources need to be added to complete parity with CloudFormation:

1. **Product Lambda Functions** (5 remaining):
   - Product materialize OpenSearch (new/update)
   - Product update notify user
   - Product enrichment ASG scale up/down

2. **API Handlers** (19 total):
   - Product: Remaining watchlist operations (partially done)
   - Shop: get, patch, search, post (4 handlers)
   - User: get account, patch account (2 handlers)
   - Search Filter: get, list, post, patch, delete (5 handlers)

3. **Product Enrichment Infrastructure**:
   - VPC (CIDR: 10.0.0.0/16)
   - 2 public subnets in different AZs
   - Internet Gateway
   - Route tables and associations
   - Security group
   - IAM role and instance profile for EC2
   - Launch template with custom AMI
   - Auto Scaling Group with mixed GPU instances (g5/g6)
   - EventBridge schedules (daily at 01:00 UTC and 03:00 UTC)

4. **EventBridge Event Rules** (6 rules):
   - DynamoDB product events to materialize queues
   - Product created/updated event routing

5. **SQS Queue Policies**:
   - Allow EventBridge to send messages to queues

6. **Missing Queue Pairs** (2 remaining):
   - Product materialize OpenSearch (new/update)
   - Product update notify user
   - Product enrichment

### Estimated Effort
Following the established patterns:
- **API Handlers**: ~2-3 hours (repetitive, same pattern)
- **Product Lambda Functions**: ~1-2 hours (repetitive, same pattern)
- **Product Enrichment VPC/ASG**: ~3-4 hours (more complex, networking + EC2)
- **EventBridge Rules**: ~1 hour (straightforward)
- **Testing**: ~2-3 hours (deploy and validate)

**Total: ~9-13 hours** to complete full parity

## Key Improvements Over CloudFormation

### 1. Reduced Duplication
**CloudFormation**: 3462 lines with lots of repetition
- Each Lambda has ~100 lines (role, function, 2 alarms, permissions)
- Each queue has ~50 lines (DLQ, queue, alarm)

**Pulumi**: Estimated ~2000 lines with abstractions
- Each Lambda: ~10-15 lines using `createLambda()`
- Each queue: ~5 lines using `createQueueWithDlq()`

### 2. Type Safety
TypeScript catches errors at compile time:
- Typos in resource names
- Invalid configuration values
- Missing required properties
- Type mismatches

### 3. Better Organization
Logical grouping into modules:
- Related resources together
- Reusable patterns extracted
- Clear separation of concerns

### 4. Easier Maintenance
Common changes become trivial:
- Add alarm: modify module once, all Lambdas get it
- Change IAM policy: update helper function, all roles updated
- Adjust queue settings: change default in one place

### 5. Testing Capability
Infrastructure can be unit tested with Pulumi testing framework (future work).

## Migration Strategy

### Phase 1: Validation (Current)
- Run `pulumi preview` to validate resource definitions
- Compare with CloudFormation resources

### Phase 2: Parallel Deployment
- Deploy Pulumi to dev/PR stacks alongside CloudFormation
- Run integration tests
- Validate functional equivalence

### Phase 3: Staged Rollout
- Switch dev/PR stacks to Pulumi
- Monitor for issues
- Switch staging to Pulumi
- Monitor for issues
- Switch production to Pulumi

### Phase 4: Cleanup
- Remove CloudFormation template
- Remove CloudFormation-specific CI/CD steps
- Update documentation

## How to Complete the Migration

### 1. Complete Remaining Resources
Follow the patterns in `index.ts` to add:
- Remaining Lambda functions (use `createLambda()`)
- API handlers (use `createApiRoute()`)
- Product enrichment infrastructure (new VPC module or inline)
- EventBridge rules (use `createEventRule()`)

### 2. Test Locally
```bash
cd pulumi
npm install
npm run build
pulumi preview
```

### 3. Deploy to Dev
```bash
pulumi config set stage dev
pulumi config set stageName pr-test
# ... other config
pulumi up
```

### 4. Validate
Run integration tests against the deployed stack.

### 5. Iterate
Fix any issues discovered and redeploy.

## Required GitHub Secrets/Variables

To deploy, configure these in GitHub repository settings:

**Secrets**:
- `PULUMI_ACCESS_TOKEN`: Pulumi Cloud access token
- `CI_DEPLOY_ROLE_ARN`: AWS IAM role for CI/CD

**Variables**:
- `AWS_REGION`: e.g., `eu-central-1`
- `S3_BINARY_ARTIFACTS_BUCKET_NAME`: Lambda deployment packages
- `S3_RESOURCE_ARTIFACTS_BUCKET_NAME`: EC2 user data scripts
- `S3_MAIL_TEMPLATE_BUCKET_NAME`: Compiled email templates
- `EC2_KEY_PAIR_NAME`: SSH key pair for enrichment instances

## Success Criteria

The migration is complete when:
- [ ] All CloudFormation resources have Pulumi equivalents
- [ ] `pulumi preview` shows no unexpected changes
- [ ] Integration tests pass against Pulumi-deployed stacks
- [ ] Dev/staging/production stacks are running on Pulumi
- [ ] CloudFormation template is archived/removed
- [ ] Team is trained on Pulumi workflows

## Conclusion

The foundation for the Pulumi migration is complete. The modular architecture and abstraction patterns make it straightforward to add the remaining resources. The biggest benefit is maintainability—future changes will be significantly easier with Pulumi's abstractions and type safety.

The established patterns in the code provide clear templates for completing the remaining work. Each new resource follows the same structure, making the completion work largely mechanical.
