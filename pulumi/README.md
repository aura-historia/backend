# Aura-Historia Backend Infrastructure - Pulumi

This directory contains the Pulumi TypeScript infrastructure code that replaces the CloudFormation template `cfn/application.yaml`.

## Why Pulumi?

Pulumi provides several advantages over plain YAML CloudFormation:

1. **Turing-Complete**: Full programming language capabilities with loops, conditionals, and functions
2. **Abstraction**: Reusable modules for common patterns (Lambda + alarms, Queue + DLQ, etc.)
3. **Type Safety**: TypeScript provides compile-time type checking
4. **Better Maintainability**: DRY principle with shared modules
5. **Modern Tooling**: IDE support, linting, formatting

## Project Structure

```
pulumi/
├── src/
│   ├── modules/          # Reusable infrastructure modules
│   │   ├── alarms.ts     # CloudWatch alarm patterns
│   │   ├── apigateway.ts # API Gateway with CORS and auth
│   │   ├── cognito.ts    # User pool and clients
│   │   ├── dynamodb.ts   # DynamoDB tables with alarms
│   │   ├── eventbridge.ts # EventBridge buses and rules
│   │   ├── iam.ts        # IAM role and policy factories
│   │   ├── lambda.ts     # Lambda function patterns
│   │   ├── opensearch.ts # OpenSearch domain with alarms
│   │   ├── queue.ts      # SQS queue + DLQ pattern
│   │   └── sns.ts        # SNS topics
│   ├── resources/        # Resource group files
│   │   ├── product-api.ts       # Product API handlers
│   │   ├── shop-api.ts          # Shop API handlers
│   │   ├── user-api.ts          # User API handlers
│   │   ├── search-filter-api.ts # Search filter API handlers
│   │   ├── product-lambda.ts    # Product processing Lambdas
│   │   └── enrichment.ts        # Product enrichment infrastructure
│   └── types.ts          # Common TypeScript types
├── index.ts              # Main Pulumi program
├── package.json          # Node.js dependencies
├── tsconfig.json         # TypeScript configuration
└── Pulumi.yaml          # Pulumi project configuration
```

## Key Abstractions

### Lambda Function Pattern

The `createLambda()` function handles common Lambda configuration:
- Automatic S3 code sourcing based on naming convention
- Standard runtime, handler, and ephemeral storage
- Optional CloudWatch error and throttle alarms
- Configurable memory and timeout

### Queue + DLQ Pattern

The `createQueueWithDlq()` function creates:
- A main SQS queue with configurable visibility timeout
- A dead letter queue with 14-day retention
- Optional CloudWatch alarm for DLQ messages

### CloudWatch Alarms

Consistent alarm patterns for:
- Lambda errors and throttles
- DynamoDB throttled requests
- OpenSearch cluster health, storage, CPU, and memory
- API Gateway 4XX and 5XX errors
- SQS DLQ messages

## Configuration

Pulumi configuration is managed through stack-specific config files and environment variables:

Required configuration values:
- `stage`: dev, staging, or prod
- `stageName`: unique name for this stage (e.g., "pr-123", "staging", "prod")
- `artifactBucket`: S3 bucket for Lambda deployment packages
- `resourceBucket`: S3 bucket for EC2 user data and other resources
- `mailTemplateBucket`: S3 bucket for email templates
- `commitSHA`: Git commit SHA for artifact versioning
- `ec2KeyPairName`: EC2 key pair for SSH access to enrichment instances

## Stage-Specific Mappings

Certain parameters vary by stage:

| Parameter | dev | staging | prod |
|-----------|-----|---------|------|
| SendMail batch window | 1s | 1s | 5s |
| Product Ingest batch window | 1s | 1s | 10s |
| DynamoDB materialization batch window | 1s | 1s | 10s |
| OpenSearch materialization batch window | 1s | 1s | 300s |
| User notification batch window | 1s | 1s | 5s |

## Resource Organization

### Core Services
- **SNS**: Alarm notification topic
- **Cognito**: User pool with public and admin clients
- **DynamoDB**: Single-table design with LSI and GSI
- **OpenSearch**: Search domain with comprehensive alarms
- **API Gateway**: HTTP API with JWT authorizer

### Lambda Functions

#### API Handlers (8 product, 4 shop, 2 user, 5 search-filter = 19 total)
- Product: get, put, search, similarity, watchlist (get/post/patch/delete)
- Shop: get, patch, search, post
- User: get account, patch account
- Search Filter: get, list, post, patch, delete

#### Processing Lambdas (6 product + 2 enrichment = 8 total)
- Product ingest events
- Product materialize DynamoDB (new/update)
- Product materialize OpenSearch (new/update)
- Product update notify user
- Product enrichment ASG scale up/down

#### System Lambdas (2 total)
- Cognito post-confirmation
- Mail send

### SQS Queues (8 with DLQs)
- Mail send
- Product ingest events
- Product materialize DynamoDB (new/update)
- Product materialize OpenSearch (new/update)
- Product update notify user
- Product enrichment

### EventBridge
- DynamoDB event bus
- Pipe from DynamoDB stream to EventBridge
- Event rules for routing product events to queues

### Product Enrichment Infrastructure
- VPC with public subnets in 2 AZs
- Internet Gateway and route tables
- Security group for EC2 instances
- Auto Scaling Group with mixed instance types (g5/g6 GPU instances)
- Launch template with custom AMI and user data
- EventBridge schedule rules for daily scaling (up at 01:00, down at 03:00 UTC)
- IAM role and instance profile with DynamoDB, OpenSearch, SQS, and S3 access

## Deployment

### Prerequisites
1. Install Pulumi CLI: https://www.pulumi.com/docs/get-started/install/
2. Install Node.js 18+ and npm
3. Configure AWS credentials

### First-Time Setup
```bash
cd pulumi
npm install
pulumi login  # Or use local/S3 backend
pulumi stack init <stage-name>
pulumi config set stage <dev|staging|prod>
pulumi config set stageName <unique-name>
pulumi config set artifactBucket <bucket-name>
pulumi config set resourceBucket <bucket-name>
pulumi config set mailTemplateBucket <bucket-name>
pulumi config set commitSHA <git-sha>
pulumi config set ec2KeyPairName <key-name>
```

### Deploy
```bash
pulumi up
```

### Preview Changes
```bash
pulumi preview
```

### Destroy Stack
```bash
pulumi destroy
```

## CI/CD Integration

The CI/CD workflow (`.github/workflows/cicd.yml`) needs to be updated to:

1. Replace `cfn-lint` job with `pulumi preview`
2. Replace `aws cloudformation deploy` with `pulumi up --yes`
3. Set Pulumi configuration from environment variables
4. Export stack outputs for testing jobs

See the updated `cicd.yml` in the repository root.

## Migration from CloudFormation

The Pulumi infrastructure is functionally equivalent to the original CloudFormation template with these improvements:

1. **Better Organization**: Resources are grouped into logical modules
2. **Less Repetition**: Common patterns are abstracted (alarms, queues, etc.)
3. **Type Safety**: TypeScript catches configuration errors at compile time
4. **Clearer Intent**: Named functions make the code self-documenting
5. **Easier Testing**: Modules can be unit tested

## Outputs

The following outputs are exported for use by other systems:

- `cognitoHostedUIDomain`: Cognito hosted UI URL
- `cognitoUserPoolId`: User pool ID
- `cognitoUserPoolClientPublicId`: Public client ID
- `cognitoUserPoolClientAdminId`: Admin client ID
- `apiGatewayEndpointUrl`: API Gateway base URL
- `opensearchDomainEndpointUrl`: OpenSearch endpoint URL
- `opensearchDomainName`: OpenSearch domain name
- `dynamodbTable1Name`: DynamoDB table name
- `alarmNotificationTopicArn`: SNS topic ARN for CloudWatch alarms
- All queue URLs and DLQ URLs

## Development

### Building
```bash
npm run build
```

### Linting
```bash
npm run lint
```

### Formatting
```bash
npm run format
```

## Troubleshooting

### Common Issues

1. **"Resource already exists"**: A resource with the same name exists in AWS. Either delete it manually or change the name.
2. **"No valid credential sources"**: Configure AWS credentials via `aws configure` or environment variables.
3. **"Type error in index.ts"**: Run `npm install` to ensure all dependencies are installed.

### Getting Help

- Pulumi Documentation: https://www.pulumi.com/docs/
- Pulumi AWS Provider: https://www.pulumi.com/registry/packages/aws/
- GitHub Issues: Report issues in the repository

## Future Improvements

Possible enhancements:
1. Split into multiple smaller stacks (networking, compute, data)
2. Add Pulumi automation API for programmatic deployments
3. Implement policy-as-code with Pulumi CrossGuard
4. Add infrastructure tests with Pulumi's testing framework
5. Use Pulumi's secrets management for sensitive values
