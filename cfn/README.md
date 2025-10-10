# CloudFormation Deployment

This directory contains the CloudFormation template for the Blitzfilter AWS Backend.

## Prerequisites

### S3 Bucket for Lambda Artifacts

The deployment requires an S3 bucket to store Lambda function deployment packages. This bucket must have **versioning enabled**.

#### Enabling S3 Versioning

If the bucket specified in `S3_BINARY_ARTIFACTS_BUCKET_NAME` (referenced as `ArtifactBucket` in CloudFormation) does not have versioning enabled, enable it using the AWS CLI:

```bash
aws s3api put-bucket-versioning \
  --bucket <bucket-name> \
  --versioning-configuration Status=Enabled
```

Or via the AWS Console:
1. Navigate to the S3 bucket
2. Go to the "Properties" tab
3. Find "Bucket Versioning" section
4. Click "Edit" and select "Enable"

#### Why Versioning is Required

The CI/CD pipeline uploads Lambda deployment packages to S3 using consistent key names (e.g., `item-api-get-item-staging.zip`). Without versioning enabled, each deployment would overwrite the previous version. With versioning:

- Each upload creates a new version of the object
- CloudFormation automatically uses the latest version when updating Lambda functions
- Previous versions are retained for rollback if needed
- No commit SHA is required in the filename, simplifying the deployment process

## Deployment

The deployment is automated via the GitHub Actions workflow (`.github/workflows/cicd.yml`). The workflow:

1. Builds all Lambda functions
2. Uploads them to the S3 bucket (with versioning)
3. Deploys/updates the CloudFormation stack

### Manual Deployment

To deploy manually:

```bash
aws cloudformation deploy \
  --stack-name <stack-name> \
  --template-file cfn/application.yaml \
  --s3-bucket <cfn-artifacts-bucket> \
  --capabilities CAPABILITY_NAMED_IAM \
  --parameter-overrides \
      Stage=<dev|staging|prod> \
      StageName=<unique-stage-name> \
      ArtifactBucket=<lambda-artifacts-bucket-name>
```

## Parameters

- `Stage`: Environment type (`dev`, `staging`, or `prod`)
- `StageName`: Unique identifier for this deployment (e.g., `pr-123`, `staging`, `prod`)
- `ArtifactBucket`: S3 bucket name containing Lambda deployment packages (must have versioning enabled)
