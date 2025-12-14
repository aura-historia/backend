/**
 * Main Pulumi program for Aura-Historia AWS Backend Infrastructure
 * 
 * This replaces the CloudFormation template cfn/application.yaml with a
 * TypeScript-based infrastructure-as-code approach using Pulumi.
 * 
 * The infrastructure is organized into logical modules for maintainability:
 * - SNS for alarm notifications
 * - Cognito for user authentication
 * - DynamoDB for data storage
 * - OpenSearch for search functionality
 * - API Gateway for HTTP API
 * - Lambda functions for business logic
 * - SQS queues for async processing
 * - EventBridge for event routing
 * - VPC and EC2 for product enrichment
 */

import * as pulumi from '@pulumi/pulumi';
import * as aws from '@pulumi/aws';
import { StackConfig, StageMapping } from './src/types';
import { createAlarmTopic } from './src/modules/sns';
import { createUserPool } from './src/modules/cognito';
import { createTable } from './src/modules/dynamodb';
import { createOpenSearchDomain } from './src/modules/opensearch';
import { createApiGateway, createApiRoute } from './src/modules/apigateway';
import { createQueueWithDlq } from './src/modules/queue';
import { createLambda, createSqsEventSourceMapping } from './src/modules/lambda';
import { createLambdaRole, dynamoDBReadPolicy, dynamoDBWritePolicy, dynamoDBFullAccessPolicy, sqsPollerPolicy, sqsSendMessagePolicy, openSearchReadPolicy, openSearchFullAccessPolicy, sesSendEmailPolicy, s3ReadPolicy } from './src/modules/iam';

// Get Pulumi configuration
const pulumiConfig = new pulumi.Config();
const stage = pulumiConfig.require('stage') as 'dev' | 'staging' | 'prod';
const stageName = pulumiConfig.require('stageName');
const artifactBucket = pulumiConfig.require('artifactBucket');
const resourceBucket = pulumiConfig.require('resourceBucket');
const mailTemplateBucket = pulumiConfig.require('mailTemplateBucket');
const commitSHA = pulumiConfig.require('commitSHA');
const ec2KeyPairName = pulumiConfig.require('ec2KeyPairName');

const config: StackConfig = {
  stage,
  stageName,
  artifactBucket,
  resourceBucket,
  mailTemplateBucket,
  commitSHA,
  ec2KeyPairName,
};

// Get AWS account and region info
const current = aws.getCallerIdentity({});
const region = aws.getRegion({});
const accountId = current.then((c) => c.accountId);
const awsRegion = region.then((r) => r.name);

// Stage-specific mappings
const sendMailQueueBatchWindow: StageMapping<number> = { dev: 1, staging: 1, prod: 5 };
const productIngestBatchWindow: StageMapping<number> = { dev: 1, staging: 1, prod: 10 };
const productMaterializeDynamoDbBatchWindow: StageMapping<number> = { dev: 1, staging: 1, prod: 10 };
const productMaterializeOpenSearchBatchWindow: StageMapping<number> = { dev: 1, staging: 1, prod: 300 };
const productUpdateNotifyUserBatchWindow: StageMapping<number> = { dev: 1, staging: 1, prod: 5 };

// ============================================================================
// SNS Topic for CloudWatch Alarms
// ============================================================================
const alarmTopic = createAlarmTopic(stageName);

// ============================================================================
// DynamoDB Table
// ============================================================================
const dynamoDb = createTable(stageName, alarmTopic.arn);

// ============================================================================
// OpenSearch Domain
// ============================================================================
const openSearch = createOpenSearchDomain(
  stageName,
  alarmTopic.arn,
  accountId.then((id) => id),
  awsRegion.then((r) => r)
);

// ============================================================================
// Cognito Post-Confirmation Lambda
// ============================================================================
const cognitoPostConfirmationRole = createLambdaRole(
  'cognito-primary-userpool-post-confirmation',
  stageName,
  [
    dynamoDBWritePolicy(dynamoDb.table.arn),
  ]
);

const cognitoPostConfirmationLambda = createLambda({
  name: 'cognito-post-confirmation',
  config,
  role: cognitoPostConfirmationRole,
  environment: {
    DYNAMODB_TABLE_NAME: dynamoDb.table.name,
  },
  memorySize: 256,
  timeout: 5,
  snsTopicArn: alarmTopic.arn,
  createAlarms: false, // No alarms for Cognito triggers
});

// Grant Cognito permission to invoke the Lambda
new aws.lambda.Permission('AllowCognitoInvokePrimaryUserPoolPostConfirmationLambda', {
  action: 'lambda:InvokeFunction',
  function: cognitoPostConfirmationLambda.lambda.name,
  principal: 'cognito-idp.amazonaws.com',
  sourceArn: pulumi.interpolate`arn:aws:cognito-idp:${awsRegion}:${accountId}:userpool/*`,
});

// ============================================================================
// Cognito User Pool
// ============================================================================
const cognito = createUserPool(stageName, cognitoPostConfirmationLambda.lambda.arn);

// ============================================================================
// API Gateway
// ============================================================================
const apiGateway = createApiGateway(
  stageName,
  cognito.userPool.id,
  cognito.publicClient.id,
  cognito.adminClient.id,
  awsRegion.then((r) => r),
  alarmTopic.arn
);

// ============================================================================
// Mail Queue and Lambda
// ============================================================================
const sendMailQueues = createQueueWithDlq({
  name: 'mail-lambda-send',
  stageName,
  snsTopicArn: alarmTopic.arn,
});

const sendMailRole = createLambdaRole('mail-lambda-send', stageName, [
  sqsPollerPolicy(sendMailQueues.queue.arn),
  sesSendEmailPolicy(),
  s3ReadPolicy(mailTemplateBucket),
]);

const sendMailLambda = createLambda({
  name: 'mail-lambda-send',
  config,
  role: sendMailRole,
  environment: {
    S3_BUCKET_NAME_TEMPLATES: mailTemplateBucket,
    STAGE_NAME: stageName,
    COMMIT_SHA: commitSHA,
  },
  timeout: 60,
  snsTopicArn: alarmTopic.arn,
  errorThreshold: 5,
});

createSqsEventSourceMapping(
  'SendMail',
  sendMailLambda.lambda,
  sendMailQueues.queue,
  200,
  sendMailQueueBatchWindow[stage]
);

// ============================================================================
// Product Ingest Queue and Lambda
// ============================================================================
const productIngestQueues = createQueueWithDlq({
  name: 'product-lambda-ingest-events-dynamodb',
  stageName,
  snsTopicArn: alarmTopic.arn,
});

const productIngestRole = createLambdaRole('product-lambda-ingest-events-dynamodb', stageName, [
  dynamoDBFullAccessPolicy(dynamoDb.table.arn),
  sqsPollerPolicy(productIngestQueues.queue.arn),
]);

const productIngestLambda = createLambda({
  name: 'product-lambda-ingest-events-dynamodb',
  config,
  role: productIngestRole,
  environment: {
    DYNAMODB_TABLE_NAME: dynamoDb.table.name,
  },
  timeout: 60,
  snsTopicArn: alarmTopic.arn,
  errorThreshold: 5,
});

createSqsEventSourceMapping(
  'ProductIngestEventsDynamoDb',
  productIngestLambda.lambda,
  productIngestQueues.queue,
  200,
  productIngestBatchWindow[stage]
);

// ============================================================================
// Product Materialize DynamoDB New Queue and Lambda
// ============================================================================
const productMaterializeDynamoDbNewQueues = createQueueWithDlq({
  name: 'product-lambda-materialize-dynamodb-new',
  stageName,
  maxReceiveCount: 5,
  snsTopicArn: alarmTopic.arn,
});

const productMaterializeDynamoDbNewRole = createLambdaRole(
  'product-lambda-materialize-dynamodb-new',
  stageName,
  [
    dynamoDBFullAccessPolicy(dynamoDb.table.arn),
    sqsPollerPolicy(productMaterializeDynamoDbNewQueues.queue.arn),
  ]
);

const productMaterializeDynamoDbNewLambda = createLambda({
  name: 'product-lambda-materialize-dynamodb-new',
  config,
  role: productMaterializeDynamoDbNewRole,
  environment: {
    DYNAMODB_TABLE_NAME: dynamoDb.table.name,
  },
  timeout: 60,
  snsTopicArn: alarmTopic.arn,
  errorThreshold: 5,
});

createSqsEventSourceMapping(
  'ProductMaterializeDynamoDbNew',
  productMaterializeDynamoDbNewLambda.lambda,
  productMaterializeDynamoDbNewQueues.queue,
  200,
  productMaterializeDynamoDbBatchWindow[stage]
);

// ============================================================================
// Product Materialize DynamoDB Update Queue and Lambda
// ============================================================================
const productMaterializeDynamoDbUpdateQueues = createQueueWithDlq({
  name: 'product-lambda-materialize-dynamodb-update',
  stageName,
  maxReceiveCount: 5,
  snsTopicArn: alarmTopic.arn,
});

const productMaterializeDynamoDbUpdateRole = createLambdaRole(
  'product-lambda-materialize-dynamodb-update',
  stageName,
  [
    dynamoDBFullAccessPolicy(dynamoDb.table.arn),
    sqsPollerPolicy(productMaterializeDynamoDbUpdateQueues.queue.arn),
  ]
);

const productMaterializeDynamoDbUpdateLambda = createLambda({
  name: 'product-lambda-materialize-dynamodb-update',
  config,
  role: productMaterializeDynamoDbUpdateRole,
  environment: {
    DYNAMODB_TABLE_NAME: dynamoDb.table.name,
  },
  timeout: 60,
  snsTopicArn: alarmTopic.arn,
  errorThreshold: 5,
});

createSqsEventSourceMapping(
  'ProductMaterializeDynamoDbUpdate',
  productMaterializeDynamoDbUpdateLambda.lambda,
  productMaterializeDynamoDbUpdateQueues.queue,
  200,
  productMaterializeDynamoDbBatchWindow[stage]
);

// Export stack outputs
export const cognitoHostedUIDomain = pulumi.interpolate`https://${cognito.domain.domain}.auth.${awsRegion}.amazoncognito.com`;
export const cognitoUserPoolId = cognito.userPool.id;
export const cognitoUserPoolClientPublicId = cognito.publicClient.id;
export const cognitoUserPoolClientAdminId = cognito.adminClient.id;
export const apiGatewayEndpointUrl = pulumi.interpolate`https://${apiGateway.api.id}.execute-api.${awsRegion}.amazonaws.com/${stageName}`;
export const opensearchDomainEndpointUrl = pulumi.interpolate`https://${openSearch.domain.endpoint}`;
export const opensearchDomainName = openSearch.domain.domainName;
export const dynamodbTable1Name = dynamoDb.table.name;
export const alarmNotificationTopicArn = alarmTopic.arn;
