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
const accountId = pulumi.output(current).apply((c) => c.accountId);
const awsRegion = pulumi.output(region).apply((r) => r.name);

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
  accountId,
  awsRegion
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
  awsRegion,
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

// ============================================================================
// Product Materialize OpenSearch New Queue and Lambda
// ============================================================================
const productMaterializeOpenSearchNewQueues = createQueueWithDlq({
  name: 'product-lambda-materialize-opensearch-new',
  stageName,
  maxReceiveCount: 5,
  snsTopicArn: alarmTopic.arn,
});

const productMaterializeOpenSearchNewRole = createLambdaRole(
  'product-lambda-materialize-opensearch-new',
  stageName,
  [
    openSearchFullAccessPolicy(openSearch.domain.arn, awsRegion, stageName),
    sqsPollerPolicy(productMaterializeOpenSearchNewQueues.queue.arn),
  ]
);

const productMaterializeOpenSearchNewLambda = createLambda({
  name: 'product-lambda-materialize-opensearch-new',
  config,
  role: productMaterializeOpenSearchNewRole,
  environment: {
    OPENSEARCH_DOMAIN_ENDPOINT_URL: pulumi.interpolate`https://${openSearch.domain.endpoint}`,
  },
  timeout: 60,
  snsTopicArn: alarmTopic.arn,
  errorThreshold: 5,
});

createSqsEventSourceMapping(
  'ProductMaterializeOpenSearchNew',
  productMaterializeOpenSearchNewLambda.lambda,
  productMaterializeOpenSearchNewQueues.queue,
  200,
  productMaterializeOpenSearchBatchWindow[stage]
);

// ============================================================================
// Product Materialize OpenSearch Update Queue and Lambda
// ============================================================================
const productMaterializeOpenSearchUpdateQueues = createQueueWithDlq({
  name: 'product-lambda-materialize-opensearch-update',
  stageName,
  maxReceiveCount: 5,
  snsTopicArn: alarmTopic.arn,
});

const productMaterializeOpenSearchUpdateRole = createLambdaRole(
  'product-lambda-materialize-opensearch-update',
  stageName,
  [
    openSearchFullAccessPolicy(openSearch.domain.arn, awsRegion, stageName),
    sqsPollerPolicy(productMaterializeOpenSearchUpdateQueues.queue.arn),
  ]
);

const productMaterializeOpenSearchUpdateLambda = createLambda({
  name: 'product-lambda-materialize-opensearch-update',
  config,
  role: productMaterializeOpenSearchUpdateRole,
  environment: {
    OPENSEARCH_DOMAIN_ENDPOINT_URL: pulumi.interpolate`https://${openSearch.domain.endpoint}`,
  },
  timeout: 60,
  snsTopicArn: alarmTopic.arn,
  errorThreshold: 5,
});

createSqsEventSourceMapping(
  'ProductMaterializeOpenSearchUpdate',
  productMaterializeOpenSearchUpdateLambda.lambda,
  productMaterializeOpenSearchUpdateQueues.queue,
  2500,
  productMaterializeOpenSearchBatchWindow[stage]
);

// ============================================================================
// Product Update Notify User Queue and Lambda
// ============================================================================
const productUpdateNotifyUserQueues = createQueueWithDlq({
  name: 'product-lambda-update-notify-user',
  stageName,
  maxReceiveCount: 5,
  snsTopicArn: alarmTopic.arn,
});

const productUpdateNotifyUserRole = createLambdaRole('product-lambda-update-notify-user', stageName, [
  dynamoDBReadPolicy(dynamoDb.table.arn),
  sqsPollerPolicy(productUpdateNotifyUserQueues.queue.arn),
  sqsSendMessagePolicy(sendMailQueues.queue.arn),
]);

const productUpdateNotifyUserLambda = createLambda({
  name: 'product-lambda-update-notify-user',
  config,
  role: productUpdateNotifyUserRole,
  environment: {
    DYNAMODB_TABLE_NAME: dynamoDb.table.name,
    MAIL_QUEUE_URL: sendMailQueues.queue.url,
    SENDER_MAIL: 'no-reply@aura-historia.com',
  },
  timeout: 60,
  snsTopicArn: alarmTopic.arn,
  errorThreshold: 5,
});

createSqsEventSourceMapping(
  'ProductUpdateNotifyUser',
  productUpdateNotifyUserLambda.lambda,
  productUpdateNotifyUserQueues.queue,
  10,
  productUpdateNotifyUserBatchWindow[stage]
);

// ============================================================================
// EventBridge Event Bus and Pipes
// ============================================================================
const dynamoDbEventBus = new aws.cloudwatch.EventBus('DynamoDbEventBus', {
  name: `dynamodb-event-bus-${stageName}`,
});

const pipeRole = new aws.iam.Role('PipeRole', {
  name: `dynamodb-to-eventbridge-pipe-role-${stageName}`,
  assumeRolePolicy: JSON.stringify({
    Version: '2012-10-17',
    Statement: [
      {
        Effect: 'Allow',
        Principal: { Service: 'pipes.amazonaws.com' },
        Action: 'sts:AssumeRole',
      },
    ],
  }),
  inlinePolicies: [
    {
      name: 'PipePolicy',
      policy: pulumi.interpolate`{
        "Version": "2012-10-17",
        "Statement": [
          {
            "Effect": "Allow",
            "Action": [
              "dynamodb:DescribeStream",
              "dynamodb:GetRecords",
              "dynamodb:GetShardIterator",
              "dynamodb:ListStreams"
            ],
            "Resource": "*"
          },
          {
            "Effect": "Allow",
            "Action": ["events:PutEvents"],
            "Resource": "${dynamoDbEventBus.arn}"
          }
        ]
      }`,
    },
  ],
});

const tableOneStreamPipe = new aws.pipes.Pipe('TableOneStreamToEventBusPipe', {
  name: `tableone-stream-to-eventbus-${stageName}`,
  roleArn: pipeRole.arn,
  source: dynamoDb.table.streamArn,
  sourceParameters: {
    dynamodbStreamParameters: {
      startingPosition: 'LATEST',
      batchSize: 10,
      maximumBatchingWindowInSeconds: 1,
    },
    filterCriteria: {
      filters: [
        {
          pattern: JSON.stringify({
            eventName: ['INSERT'],
            dynamodb: {
              NewImage: {
                sk: {
                  S: [{ prefix: 'product#event#' }],
                },
              },
            },
          }),
        },
      ],
    },
  },
  target: dynamoDbEventBus.arn,
  targetParameters: {
    eventbridgeEventBusParameters: {
      detailType: 'DynamoDBStreamRecord',
      source: dynamoDb.table.name,
    },
  },
});

// SQS Queue Policies for EventBridge
const productMatDbNewQueuePolicy = new aws.sqs.QueuePolicy('ProductMatDbNewQueuePolicy', {
  queueUrl: productMaterializeDynamoDbNewQueues.queue.url,
  policy: productMaterializeDynamoDbNewQueues.queue.arn.apply((arn) =>
    JSON.stringify({
      Version: '2012-10-17',
      Statement: [
        {
          Effect: 'Allow',
          Principal: { Service: 'events.amazonaws.com' },
          Action: 'sqs:SendMessage',
          Resource: arn,
        },
      ],
    })
  ),
});

const productMatDbUpdateQueuePolicy = new aws.sqs.QueuePolicy('ProductMatDbUpdateQueuePolicy', {
  queueUrl: productMaterializeDynamoDbUpdateQueues.queue.url,
  policy: productMaterializeDynamoDbUpdateQueues.queue.arn.apply((arn) =>
    JSON.stringify({
      Version: '2012-10-17',
      Statement: [
        {
          Effect: 'Allow',
          Principal: { Service: 'events.amazonaws.com' },
          Action: 'sqs:SendMessage',
          Resource: arn,
        },
      ],
    })
  ),
});

const productMatOsNewQueuePolicy = new aws.sqs.QueuePolicy('ProductMatOsNewQueuePolicy', {
  queueUrl: productMaterializeOpenSearchNewQueues.queue.url,
  policy: productMaterializeOpenSearchNewQueues.queue.arn.apply((arn) =>
    JSON.stringify({
      Version: '2012-10-17',
      Statement: [
        {
          Effect: 'Allow',
          Principal: { Service: 'events.amazonaws.com' },
          Action: 'sqs:SendMessage',
          Resource: arn,
        },
      ],
    })
  ),
});

const productMatOsUpdateQueuePolicy = new aws.sqs.QueuePolicy('ProductMatOsUpdateQueuePolicy', {
  queueUrl: productMaterializeOpenSearchUpdateQueues.queue.url,
  policy: productMaterializeOpenSearchUpdateQueues.queue.arn.apply((arn) =>
    JSON.stringify({
      Version: '2012-10-17',
      Statement: [
        {
          Effect: 'Allow',
          Principal: { Service: 'events.amazonaws.com' },
          Action: 'sqs:SendMessage',
          Resource: arn,
        },
      ],
    })
  ),
});

const productNotifyUserQueuePolicy = new aws.sqs.QueuePolicy('ProductNotifyUserQueuePolicy', {
  queueUrl: productUpdateNotifyUserQueues.queue.url,
  policy: productUpdateNotifyUserQueues.queue.arn.apply((arn) =>
    JSON.stringify({
      Version: '2012-10-17',
      Statement: [
        {
          Effect: 'Allow',
          Principal: { Service: 'events.amazonaws.com' },
          Action: 'sqs:SendMessage',
          Resource: arn,
        },
      ],
    })
  ),
});

// EventBridge Rules for routing product events
const productCreatedMatDbRule = new aws.cloudwatch.EventRule('DdbProductMatDbNewRule', {
  name: `ddb-product-mat-ddb-new-${stageName}`,
  eventBusName: dynamoDbEventBus.name,
  eventPattern: pulumi.interpolate`{
    "source": ["${dynamoDb.table.name}"],
    "detail-type": ["DynamoDBStreamRecord"],
    "detail": {
      "eventName": ["INSERT"],
      "dynamodb": {
        "NewImage": {
          "event_type": {
            "S": ["CREATED"]
          }
        }
      }
    }
  }`,
});

new aws.cloudwatch.EventTarget('DdbProductMatDbNewTarget', {
  rule: productCreatedMatDbRule.name,
  eventBusName: dynamoDbEventBus.name,
  arn: productMaterializeDynamoDbNewQueues.queue.arn,
});

const productUpdatedMatDbRule = new aws.cloudwatch.EventRule('DdbProductMatDbUpdateRule', {
  name: `ddb-product-mat-ddb-update-${stageName}`,
  eventBusName: dynamoDbEventBus.name,
  eventPattern: pulumi.interpolate`{
    "source": ["${dynamoDb.table.name}"],
    "detail-type": ["DynamoDBStreamRecord"],
    "detail": {
      "eventName": ["INSERT"],
      "dynamodb": {
        "NewImage": {
          "event_type": {
            "S": [{"prefix": "PRICE_"}, {"prefix": "STATE_"}]
          }
        }
      }
    }
  }`,
});

new aws.cloudwatch.EventTarget('DdbProductMatDbUpdateTarget', {
  rule: productUpdatedMatDbRule.name,
  eventBusName: dynamoDbEventBus.name,
  arn: productMaterializeDynamoDbUpdateQueues.queue.arn,
});

const productCreatedMatOsRule = new aws.cloudwatch.EventRule('DdbProductMatOsNewRule', {
  name: `ddb-product-mat-os-new-${stageName}`,
  eventBusName: dynamoDbEventBus.name,
  eventPattern: pulumi.interpolate`{
    "source": ["${dynamoDb.table.name}"],
    "detail-type": ["DynamoDBStreamRecord"],
    "detail": {
      "eventName": ["INSERT"],
      "dynamodb": {
        "NewImage": {
          "event_type": {
            "S": ["CREATED"]
          }
        }
      }
    }
  }`,
});

new aws.cloudwatch.EventTarget('DdbProductMatOsNewTarget', {
  rule: productCreatedMatOsRule.name,
  eventBusName: dynamoDbEventBus.name,
  arn: productMaterializeOpenSearchNewQueues.queue.arn,
});

const productUpdatedMatOsRule = new aws.cloudwatch.EventRule('DdbProductMatOsUpdateRule', {
  name: `ddb-product-mat-os-update-${stageName}`,
  eventBusName: dynamoDbEventBus.name,
  eventPattern: pulumi.interpolate`{
    "source": ["${dynamoDb.table.name}"],
    "detail-type": ["DynamoDBStreamRecord"],
    "detail": {
      "eventName": ["INSERT"],
      "dynamodb": {
        "NewImage": {
          "event_type": {
            "S": [{"prefix": "PRICE_"}, {"prefix": "STATE_"}]
          }
        }
      }
    }
  }`,
});

new aws.cloudwatch.EventTarget('DdbProductMatOsUpdateTarget', {
  rule: productUpdatedMatOsRule.name,
  eventBusName: dynamoDbEventBus.name,
  arn: productMaterializeOpenSearchUpdateQueues.queue.arn,
});

const productUpdatedNotifyUserRule = new aws.cloudwatch.EventRule('DdbProductNotifyUserRule', {
  name: `ddb-product-update-notify-user-${stageName}`,
  eventBusName: dynamoDbEventBus.name,
  eventPattern: pulumi.interpolate`{
    "source": ["${dynamoDb.table.name}"],
    "detail-type": ["DynamoDBStreamRecord"],
    "detail": {
      "eventName": ["INSERT"],
      "dynamodb": {
        "NewImage": {
          "event_type": {
            "S": [{"prefix": "PRICE_"}, {"prefix": "STATE_"}]
          }
        }
      }
    }
  }`,
});

new aws.cloudwatch.EventTarget('DdbProductNotifyUserTarget', {
  rule: productUpdatedNotifyUserRule.name,
  eventBusName: dynamoDbEventBus.name,
  arn: productUpdateNotifyUserQueues.queue.arn,
});

// ============================================================================
// Product Enrichment Infrastructure
// ============================================================================

// VPC
const productEnrichmentVpc = new aws.ec2.Vpc('ProductEnrichmentVpc', {
  cidrBlock: '10.0.0.0/16',
  enableDnsSupport: true,
  enableDnsHostnames: true,
  tags: { Name: `product-enrichment-vpc-${stageName}` },
});

// Internet Gateway
const productEnrichmentIgw = new aws.ec2.InternetGateway('ProductEnrichmentInternetGateway', {
  vpcId: productEnrichmentVpc.id,
  tags: { Name: `product-enrichment-igw-${stageName}` },
});

// Route Table
const productEnrichmentRouteTable = new aws.ec2.RouteTable('ProductEnrichmentPublicRouteTable', {
  vpcId: productEnrichmentVpc.id,
  tags: { Name: `product-enrichment-public-rt-${stageName}` },
});

// Public Route
const productEnrichmentPublicRoute = new aws.ec2.Route('ProductEnrichmentPublicRoute', {
  routeTableId: productEnrichmentRouteTable.id,
  destinationCidrBlock: '0.0.0.0/0',
  gatewayId: productEnrichmentIgw.id,
});

// Subnets
const availabilityZones = aws.getAvailabilityZones({ state: 'available' });

const productEnrichmentSubnetA = new aws.ec2.Subnet('ProductEnrichmentSubnetA', {
  vpcId: productEnrichmentVpc.id,
  cidrBlock: '10.0.1.0/24',
  mapPublicIpOnLaunch: true,
  availabilityZone: availabilityZones.then((azs) => azs.names[0]),
  tags: { Name: `product-enrichment-subnet-a-${stageName}` },
});

const productEnrichmentSubnetB = new aws.ec2.Subnet('ProductEnrichmentSubnetB', {
  vpcId: productEnrichmentVpc.id,
  cidrBlock: '10.0.2.0/24',
  mapPublicIpOnLaunch: true,
  availabilityZone: availabilityZones.then((azs) => azs.names[1]),
  tags: { Name: `product-enrichment-subnet-b-${stageName}` },
});

// Route Table Associations
new aws.ec2.RouteTableAssociation('ProductEnrichmentSubnetRouteAssocA', {
  subnetId: productEnrichmentSubnetA.id,
  routeTableId: productEnrichmentRouteTable.id,
});

new aws.ec2.RouteTableAssociation('ProductEnrichmentSubnetRouteAssocB', {
  subnetId: productEnrichmentSubnetB.id,
  routeTableId: productEnrichmentRouteTable.id,
});

// Security Group
const productEnrichmentSecurityGroup = new aws.ec2.SecurityGroup('ProductEnrichmentSecurityGroup', {
  vpcId: productEnrichmentVpc.id,
  description: 'Allow SSH and internal comms',
  egress: [{ protocol: '-1', fromPort: 0, toPort: 0, cidrBlocks: ['0.0.0.0/0'] }],
  tags: { Name: `product-enrichment-sg-${stageName}` },
});

// Product Enrichment Queue
const productEnrichmentQueues = createQueueWithDlq({
  name: 'product-enrichment',
  stageName,
  maxReceiveCount: 3,
  snsTopicArn: alarmTopic.arn,
});

// Product Enrichment Queue Policy
const enrichmentQueuePolicy = new aws.sqs.QueuePolicy('ProductEnrichmentQueuePolicy', {
  queueUrl: productEnrichmentQueues.queue.url,
  policy: productEnrichmentQueues.queue.arn.apply((arn) =>
    JSON.stringify({
      Version: '2012-10-17',
      Statement: [
        {
          Effect: 'Allow',
          Principal: { Service: 'events.amazonaws.com' },
          Action: 'sqs:SendMessage',
          Resource: arn,
        },
      ],
    })
  ),
});

// EventBridge Rule for Product Enrichment
const productCreatedEnrichmentRule = new aws.cloudwatch.EventRule('DdbProductEnrichmentRule', {
  name: `ddb-product-enrichment-${stageName}`,
  eventBusName: dynamoDbEventBus.name,
  eventPattern: pulumi.interpolate`{
    "source": ["${dynamoDb.table.name}"],
    "detail-type": ["DynamoDBStreamRecord"],
    "detail": {
      "eventName": ["INSERT"],
      "dynamodb": {
        "NewImage": {
          "event_type": {
            "S": ["CREATED"]
          }
        }
      }
    }
  }`,
});

new aws.cloudwatch.EventTarget('DdbProductEnrichmentTarget', {
  rule: productCreatedEnrichmentRule.name,
  eventBusName: dynamoDbEventBus.name,
  arn: productEnrichmentQueues.queue.arn,
});

// IAM Role for EC2 Product Enrichment
const productEnrichmentRole = new aws.iam.Role('ProductEnrichmentRole', {
  name: `product-enrichment-role-${stageName}`,
  assumeRolePolicy: JSON.stringify({
    Version: '2012-10-17',
    Statement: [
      {
        Effect: 'Allow',
        Principal: { Service: ['ec2.amazonaws.com', 'spot.amazonaws.com'] },
        Action: 'sts:AssumeRole',
      },
    ],
  }),
  managedPolicyArns: [
    'arn:aws:iam::aws:policy/CloudWatchAgentServerPolicy',
    'arn:aws:iam::aws:policy/AmazonS3ReadOnlyAccess',
    'arn:aws:iam::aws:policy/AmazonSSMManagedInstanceCore',
  ],
  inlinePolicies: [
    {
      name: 'AllowTerminateInASG',
      policy: JSON.stringify({
        Version: '2012-10-17',
        Statement: [
          {
            Effect: 'Allow',
            Action: 'autoscaling:TerminateInstanceInAutoScalingGroup',
            Resource: '*',
          },
        ],
      }),
    },
    dynamoDBFullAccessPolicy(dynamoDb.table.arn),
    openSearchFullAccessPolicy(openSearch.domain.arn, awsRegion, stageName),
    sqsPollerPolicy(productEnrichmentQueues.queue.arn),
    {
      name: 'SQSDeleteBatch',
      policy: productEnrichmentQueues.queue.arn.apply((arn) =>
        JSON.stringify({
          Version: '2012-10-17',
          Statement: [
            {
              Effect: 'Allow',
              Action: ['sqs:DeleteMessageBatch'],
              Resource: arn,
            },
          ],
        })
      ),
    },
    {
      name: 'CloudWatchLogsCreate',
      policy: JSON.stringify({
        Version: '2012-10-17',
        Statement: [
          {
            Effect: 'Allow',
            Action: ['logs:CreateLogGroup', 'logs:CreateLogStream', 'logs:PutLogEvents'],
            Resource: '*',
          },
        ],
      }),
    },
  ],
});

// Instance Profile
const productEnrichmentInstanceProfile = new aws.iam.InstanceProfile('ProductEnrichmentInstanceProfile', {
  name: `product-enrichment-profile-${stageName}`,
  role: productEnrichmentRole.name,
});

// Launch Template
const productEnrichmentLaunchTemplate = new aws.ec2.LaunchTemplate('ProductEnrichmentLaunchTemplate', {
  name: `product-enrichment-template-${stageName}`,
  keyName: ec2KeyPairName,
  iamInstanceProfile: { name: productEnrichmentInstanceProfile.name },
  instanceType: 'g6.xlarge',
  imageId: 'ami-028f346ccb9f0f3c9', // custom eu-central-1 AMI
  vpcSecurityGroupIds: [productEnrichmentSecurityGroup.id],
  blockDeviceMappings: [
    {
      deviceName: '/dev/sda1',
      ebs: {
        volumeSize: 100,
        volumeType: 'gp3',
        deleteOnTermination: 'true',
      },
    },
  ],
  tagSpecifications: [
    {
      resourceType: 'instance',
      tags: {
        Name: `product-enrichment-${stageName}`,
        Fleet: 'product-enrichment',
      },
    },
  ],
  userData: pulumi
    .all([stageName, commitSHA, resourceBucket, artifactBucket, productEnrichmentQueues.queue.url, dynamoDb.table.name, openSearch.domain.endpoint])
    .apply(([sn, sha, resBucket, artBucket, queueUrl, tableName, osEndpoint]) =>
      Buffer.from(
        `#!/bin/bash
set -euxo pipefail
sudo -i
export STAGE_NAME="${sn}"
export COMMIT_SHA="${sha}"
export RESOURCE_BUCKET="${resBucket}"
export ARTIFACT_BUCKET="${artBucket}"
export ENRICHMENT_QUEUE_URL="${queueUrl}"
export DYNAMODB_TABLE_NAME="${tableName}"
export OPENSEARCH_DOMAIN_ENDPOINT_URL="https://${osEndpoint}"
export BAAI_BGE_M3_MODEL_DEVICE="cuda"
aws s3 cp "s3://${resBucket}/${sn}/${sha}/src/product-enrichment/user-data.sh" /tmp/user_data.sh --region eu-central-1
source /tmp/user_data.sh`
      ).toString('base64')
    ),
});

// Auto Scaling Group
const productEnrichmentASG = new aws.autoscaling.Group('ProductEnrichmentASG', {
  vpcZoneIdentifiers: [productEnrichmentSubnetA.id, productEnrichmentSubnetB.id],
  minSize: 0,
  maxSize: 3,
  desiredCapacity: 0,
  mixedInstancesPolicy: {
    launchTemplate: {
      launchTemplateSpecification: {
        launchTemplateId: productEnrichmentLaunchTemplate.id,
        version: '$Latest',
      },
      overrides: [
        { instanceType: 'g6.xlarge' },
        { instanceType: 'g6.2xlarge' },
        { instanceType: 'g5.xlarge' },
        { instanceType: 'g5.2xlarge' },
      ],
    },
    instancesDistribution: {
      onDemandPercentageAboveBaseCapacity: 0,
      spotAllocationStrategy: 'price-capacity-optimized',
    },
  },
  tags: [
    {
      key: 'Name',
      value: `product-enrichment-${stageName}`,
      propagateAtLaunch: true,
    },
  ],
  healthCheckType: 'EC2',
  healthCheckGracePeriod: 120,
});

// ASG Alarms
const asgInstanceStatusAlarm = new aws.cloudwatch.MetricAlarm('ProductEnrichmentASGEC2InstanceStatusCheckFailed', {
  alarmDescription: 'One or more instance status checks are failing.',
  namespace: 'AWS/EC2',
  metricName: 'StatusCheckFailed_Instance',
  dimensions: {
    AutoScalingGroupName: productEnrichmentASG.name,
  },
  statistic: 'Maximum',
  period: 60,
  evaluationPeriods: 2,
  threshold: 1,
  comparisonOperator: 'GreaterThanOrEqualToThreshold',
  treatMissingData: 'notBreaching',
  alarmActions: [alarmTopic.arn],
});

const asgSystemStatusAlarm = new aws.cloudwatch.MetricAlarm('ProductEnrichmentASGEC2SystemStatusCheckFailed', {
  alarmDescription: 'System status check failed (hardware/hypervisor).',
  namespace: 'AWS/EC2',
  metricName: 'StatusCheckFailed_System',
  dimensions: {
    AutoScalingGroupName: productEnrichmentASG.name,
  },
  statistic: 'Maximum',
  period: 60,
  evaluationPeriods: 2,
  threshold: 1,
  comparisonOperator: 'GreaterThanOrEqualToThreshold',
  treatMissingData: 'notBreaching',
  alarmActions: [alarmTopic.arn],
});

// Scale Up Lambda
const productEnrichmentScaleUpRole = createLambdaRole('product-enrichment-asg-scale-up', stageName, [
  {
    name: 'ScaleUpASG',
    policy: pulumi.interpolate`{
      "Version": "2012-10-17",
      "Statement": [{
        "Effect": "Allow",
        "Action": ["autoscaling:UpdateAutoScalingGroup", "autoscaling:DescribeAutoScalingGroups"],
        "Resource": "arn:aws:autoscaling:${awsRegion}:${accountId}:autoScalingGroup:*:autoScalingGroupName/${productEnrichmentASG.name}"
      }]
    }`,
  },
  openSearchFullAccessPolicy(openSearch.domain.arn, awsRegion, stageName),
]);

const productEnrichmentScaleUpLambda = createLambda({
  name: 'product-enrichment-asg-scale-up',
  config,
  role: productEnrichmentScaleUpRole,
  environment: {
    PRODUCT_ENRICHMENT_ASG_NAME: productEnrichmentASG.name,
    OPENSEARCH_DOMAIN_ENDPOINT_URL: pulumi.interpolate`https://${openSearch.domain.endpoint}`,
  },
  memorySize: 128,
  timeout: 60,
  snsTopicArn: alarmTopic.arn,
});

// Scale Up Schedule
const scaleUpSchedule = new aws.cloudwatch.EventRule('ProductEnrichmentScaleUpStartSchedule', {
  scheduleExpression: 'cron(0 1 * * ? *)', // 01:00 UTC daily
});

new aws.cloudwatch.EventTarget('ProductEnrichmentScaleUpStartScheduleTarget', {
  rule: scaleUpSchedule.name,
  arn: productEnrichmentScaleUpLambda.lambda.arn,
  retryPolicy: {
    maximumRetryAttempts: 5,
    maximumEventAgeInSeconds: 3600,
  },
  deadLetterConfig: {
    arn: productEnrichmentQueues.dlq.arn,
  },
});

new aws.lambda.Permission('ProductEnrichmentScaleUpStartSchedulePermission', {
  action: 'lambda:InvokeFunction',
  function: productEnrichmentScaleUpLambda.lambda.name,
  principal: 'events.amazonaws.com',
  sourceArn: scaleUpSchedule.arn,
});

// Scale Down Lambda
const productEnrichmentScaleDownRole = createLambdaRole('product-enrichment-asg-scale-down', stageName, [
  {
    name: 'ScaleDownASG',
    policy: pulumi.interpolate`{
      "Version": "2012-10-17",
      "Statement": [{
        "Effect": "Allow",
        "Action": ["autoscaling:UpdateAutoScalingGroup", "autoscaling:DescribeAutoScalingGroups"],
        "Resource": "arn:aws:autoscaling:${awsRegion}:${accountId}:autoScalingGroup:*:autoScalingGroupName/${productEnrichmentASG.name}"
      }]
    }`,
  },
  openSearchFullAccessPolicy(openSearch.domain.arn, awsRegion, stageName),
]);

const productEnrichmentScaleDownLambda = createLambda({
  name: 'product-enrichment-asg-scale-down',
  config,
  role: productEnrichmentScaleDownRole,
  environment: {
    PRODUCT_ENRICHMENT_ASG_NAME: productEnrichmentASG.name,
    OPENSEARCH_DOMAIN_ENDPOINT_URL: pulumi.interpolate`https://${openSearch.domain.endpoint}`,
  },
  memorySize: 128,
  timeout: 60,
  snsTopicArn: alarmTopic.arn,
});

// Scale Down Schedule
const scaleDownSchedule = new aws.cloudwatch.EventRule('ProductEnrichmentScaleDownStartSchedule', {
  scheduleExpression: 'cron(0 3 * * ? *)', // 03:00 UTC daily
});

new aws.cloudwatch.EventTarget('ProductEnrichmentScaleDownStartScheduleTarget', {
  rule: scaleDownSchedule.name,
  arn: productEnrichmentScaleDownLambda.lambda.arn,
  retryPolicy: {
    maximumRetryAttempts: 5,
    maximumEventAgeInSeconds: 3600,
  },
  deadLetterConfig: {
    arn: productEnrichmentQueues.dlq.arn,
  },
});

new aws.lambda.Permission('ProductEnrichmentScaleDownStartSchedulePermission', {
  action: 'lambda:InvokeFunction',
  function: productEnrichmentScaleDownLambda.lambda.name,
  principal: 'events.amazonaws.com',
  sourceArn: scaleDownSchedule.arn,
});

// ============================================================================
// Product API Handlers
// ============================================================================

// API Get Product
const apiGetProductRole = createLambdaRole('api-get-product', stageName, [dynamoDBReadPolicy(dynamoDb.table.arn)]);

const apiGetProductLambda = createLambda({
  name: 'product-api-get-product',
  config,
  role: apiGetProductRole,
  environment: {
    DYNAMODB_TABLE_NAME: dynamoDb.table.name,
    USER_POOL_ID: cognito.userPool.id,
    USER_POOL_PUBLIC_CLIENT_ID: cognito.publicClient.id,
    USER_POOL_ADMIN_CLIENT_ID: cognito.adminClient.id,
  },
  snsTopicArn: alarmTopic.arn,
});

createApiRoute('ApiGetProduct', {
  api: apiGateway.api,
  routeKey: 'GET /api/v1/products/{shopId}/{shopsProductId}',
  lambda: apiGetProductLambda.lambda,
  accountId: accountId,
  region: awsRegion,
  routePattern: '/api/v1/products/*/*',
});

// API Put Products
const apiPutProductsRole = createLambdaRole('api-put-products', stageName, [
  dynamoDBReadPolicy(dynamoDb.table.arn),
  sqsSendMessagePolicy(productIngestQueues.queue.arn),
]);

const apiPutProductsLambda = createLambda({
  name: 'product-api-put-products',
  config,
  role: apiPutProductsRole,
  environment: {
    DYNAMODB_TABLE_NAME: dynamoDb.table.name,
    INGEST_PRODUCT_EVENTS_QUEUE_URL: productIngestQueues.queue.url,
  },
  memorySize: 1024,
  timeout: 60,
  snsTopicArn: alarmTopic.arn,
});

createApiRoute('ApiPutProducts', {
  api: apiGateway.api,
  routeKey: 'PUT /api/v1/products',
  lambda: apiPutProductsLambda.lambda,
  accountId: accountId,
  region: awsRegion,
  routePattern: '/api/v1/products',
});

// API Product Search
const apiProductSearchRole = createLambdaRole('api-product-search', stageName, [
  openSearchReadPolicy(openSearch.domain.arn, awsRegion, stageName),
  dynamoDBReadPolicy(dynamoDb.table.arn),
]);

const apiProductSearchLambda = createLambda({
  name: 'product-api-search',
  config,
  role: apiProductSearchRole,
  environment: {
    OPENSEARCH_DOMAIN_ENDPOINT_URL: pulumi.interpolate`https://${openSearch.domain.endpoint}`,
    DYNAMODB_TABLE_NAME: dynamoDb.table.name,
    USER_POOL_ID: cognito.userPool.id,
    USER_POOL_PUBLIC_CLIENT_ID: cognito.publicClient.id,
    USER_POOL_ADMIN_CLIENT_ID: cognito.adminClient.id,
  },
  snsTopicArn: alarmTopic.arn,
});

createApiRoute('ApiProductSearch', {
  api: apiGateway.api,
  routeKey: 'POST /api/v1/products/search',
  lambda: apiProductSearchLambda.lambda,
  accountId: accountId,
  region: awsRegion,
  routePattern: '/api/v1/products/search*',
});

// API Product Similar
const apiProductSimilarRole = createLambdaRole('api-product-similarity-search', stageName, [
  openSearchReadPolicy(openSearch.domain.arn, awsRegion, stageName),
  dynamoDBReadPolicy(dynamoDb.table.arn),
]);

const apiProductSimilarLambda = createLambda({
  name: 'product-api-get-product-similar',
  config,
  role: apiProductSimilarRole,
  environment: {
    OPENSEARCH_DOMAIN_ENDPOINT_URL: pulumi.interpolate`https://${openSearch.domain.endpoint}`,
    DYNAMODB_TABLE_NAME: dynamoDb.table.name,
    USER_POOL_ID: cognito.userPool.id,
    USER_POOL_PUBLIC_CLIENT_ID: cognito.publicClient.id,
    USER_POOL_ADMIN_CLIENT_ID: cognito.adminClient.id,
  },
  snsTopicArn: alarmTopic.arn,
});

createApiRoute('ApiProductSimilar', {
  api: apiGateway.api,
  routeKey: 'GET /api/v1/products/{shopId}/{shopsProductId}/similar',
  lambda: apiProductSimilarLambda.lambda,
  accountId: accountId,
  region: awsRegion,
  routePattern: '/api/v1/products/*/*/*',
});

// API Watchlist Get
const apiWatchlistGetRole = createLambdaRole('api-get-watchlist-products', stageName, [
  dynamoDBReadPolicy(dynamoDb.table.arn),
]);

const apiWatchlistGetLambda = createLambda({
  name: 'product-api-watchlist-get',
  config,
  role: apiWatchlistGetRole,
  environment: {
    DYNAMODB_TABLE_NAME: dynamoDb.table.name,
  },
  snsTopicArn: alarmTopic.arn,
  createAlarms: false,
});

createApiRoute('ApiWatchlistGet', {
  api: apiGateway.api,
  routeKey: 'GET /api/v1/me/watchlist',
  lambda: apiWatchlistGetLambda.lambda,
  authorizerId: apiGateway.authorizer.id,
  accountId: accountId,
  region: awsRegion,
  routePattern: '/api/v1/me/watchlist*',
});

// API Watchlist Delete
const apiWatchlistDeleteRole = createLambdaRole('api-delete-watchlist-product', stageName, [
  {
    name: 'DynamoDBAccess',
    policy: pulumi.interpolate`{
      "Version": "2012-10-17",
      "Statement": [{
        "Effect": "Allow",
        "Action": ["dynamodb:GetItem", "dynamodb:BatchGetItem", "dynamodb:Query", "dynamodb:DeleteItem"],
        "Resource": "${dynamoDb.table.arn}"
      }]
    }`,
  },
]);

const apiWatchlistDeleteLambda = createLambda({
  name: 'product-api-watchlist-delete',
  config,
  role: apiWatchlistDeleteRole,
  environment: {
    DYNAMODB_TABLE_NAME: dynamoDb.table.name,
  },
  snsTopicArn: alarmTopic.arn,
  createAlarms: false,
});

createApiRoute('ApiWatchlistDelete', {
  api: apiGateway.api,
  routeKey: 'DELETE /api/v1/me/watchlist/{shopId}/{shopsProductId}',
  lambda: apiWatchlistDeleteLambda.lambda,
  authorizerId: apiGateway.authorizer.id,
  accountId: accountId,
  region: awsRegion,
  routePattern: '/api/v1/me/watchlist/*/*',
});

// API Watchlist Post
const apiWatchlistPostRole = createLambdaRole('api-post-watchlist-product', stageName, [
  {
    name: 'DynamoDBAccess',
    policy: pulumi.interpolate`{
      "Version": "2012-10-17",
      "Statement": [{
        "Effect": "Allow",
        "Action": ["dynamodb:GetItem", "dynamodb:BatchGetItem", "dynamodb:Query", "dynamodb:PutItem"],
        "Resource": "${dynamoDb.table.arn}"
      }]
    }`,
  },
]);

const apiWatchlistPostLambda = createLambda({
  name: 'product-api-watchlist-post',
  config,
  role: apiWatchlistPostRole,
  environment: {
    DYNAMODB_TABLE_NAME: dynamoDb.table.name,
  },
  snsTopicArn: alarmTopic.arn,
  createAlarms: false,
});

createApiRoute('ApiWatchlistPost', {
  api: apiGateway.api,
  routeKey: 'POST /api/v1/me/watchlist',
  lambda: apiWatchlistPostLambda.lambda,
  authorizerId: apiGateway.authorizer.id,
  accountId: accountId,
  region: awsRegion,
  routePattern: '/api/v1/me/watchlist',
});

// API Watchlist Patch
const apiWatchlistPatchRole = createLambdaRole('api-patch-watchlist-product', stageName, [
  {
    name: 'DynamoDBAccess',
    policy: pulumi.interpolate`{
      "Version": "2012-10-17",
      "Statement": [{
        "Effect": "Allow",
        "Action": ["dynamodb:GetItem", "dynamodb:UpdateItem"],
        "Resource": "${dynamoDb.table.arn}"
      }]
    }`,
  },
]);

const apiWatchlistPatchLambda = createLambda({
  name: 'product-api-watchlist-patch',
  config,
  role: apiWatchlistPatchRole,
  environment: {
    DYNAMODB_TABLE_NAME: dynamoDb.table.name,
  },
  memorySize: 256,
  snsTopicArn: alarmTopic.arn,
  createAlarms: false,
});

createApiRoute('ApiWatchlistPatch', {
  api: apiGateway.api,
  routeKey: 'PATCH /api/v1/me/watchlist/{shopId}/{shopsProductId}',
  lambda: apiWatchlistPatchLambda.lambda,
  authorizerId: apiGateway.authorizer.id,
  accountId: accountId,
  region: awsRegion,
  routePattern: '/api/v1/me/watchlist/*/*',
});

// ============================================================================
// Shop API Handlers
// ============================================================================

// API Get Shop
const apiGetShopRole = createLambdaRole('api-get-shop', stageName, [dynamoDBReadPolicy(dynamoDb.table.arn)]);

const apiGetShopLambda = createLambda({
  name: 'shop-api-get-shop',
  config,
  role: apiGetShopRole,
  environment: {
    DYNAMODB_TABLE_NAME: dynamoDb.table.name,
  },
  snsTopicArn: alarmTopic.arn,
  createAlarms: false,
});

createApiRoute('ApiGetShop', {
  api: apiGateway.api,
  routeKey: 'GET /api/v1/shops/{shopId}',
  lambda: apiGetShopLambda.lambda,
  accountId: accountId,
  region: awsRegion,
  routePattern: '/api/v1/shops/*',
});

// API Patch Shop
const apiPatchShopRole = createLambdaRole('api-patch-shop', stageName, [
  {
    name: 'DynamoDBAccess',
    policy: pulumi.interpolate`{
      "Version": "2012-10-17",
      "Statement": [{
        "Effect": "Allow",
        "Action": [
          "dynamodb:Query", "dynamodb:GetItem", "dynamodb:PutItem",
          "dynamodb:UpdateItem", "dynamodb:DeleteItem", "dynamodb:BatchGetItem",
          "dynamodb:BatchWriteItem", "dynamodb:TransactGetItems", "dynamodb:TransactWriteItems"
        ],
        "Resource": "${dynamoDb.table.arn}"
      }]
    }`,
  },
]);

const apiPatchShopLambda = createLambda({
  name: 'shop-api-patch-shop',
  config,
  role: apiPatchShopRole,
  environment: {
    DYNAMODB_TABLE_NAME: dynamoDb.table.name,
  },
  snsTopicArn: alarmTopic.arn,
  createAlarms: false,
});

createApiRoute('ApiPatchShop', {
  api: apiGateway.api,
  routeKey: 'PATCH /api/v1/shops/{shopId}',
  lambda: apiPatchShopLambda.lambda,
  accountId: accountId,
  region: awsRegion,
  routePattern: '/api/v1/shops/*',
});

// API Search Shops
const apiShopSearchRole = createLambdaRole('api-shop-search', stageName, [
  openSearchReadPolicy(openSearch.domain.arn, awsRegion, stageName),
  dynamoDBReadPolicy(dynamoDb.table.arn),
]);

const apiShopSearchLambda = createLambda({
  name: 'shop-api-search',
  config,
  role: apiShopSearchRole,
  environment: {
    OPENSEARCH_DOMAIN_ENDPOINT_URL: pulumi.interpolate`https://${openSearch.domain.endpoint}`,
    DYNAMODB_TABLE_NAME: dynamoDb.table.name,
  },
  snsTopicArn: alarmTopic.arn,
  createAlarms: false,
});

createApiRoute('ApiShopSearch', {
  api: apiGateway.api,
  routeKey: 'POST /api/v1/shops/search',
  lambda: apiShopSearchLambda.lambda,
  accountId: accountId,
  region: awsRegion,
  routePattern: '/api/v1/shops/search*',
});

// API Post Shop
const apiPostShopRole = createLambdaRole('api-post-shop', stageName, [
  {
    name: 'DynamoDBAccess',
    policy: pulumi.interpolate`{
      "Version": "2012-10-17",
      "Statement": [{
        "Effect": "Allow",
        "Action": [
          "dynamodb:Query", "dynamodb:GetItem", "dynamodb:PutItem",
          "dynamodb:UpdateItem", "dynamodb:DeleteItem", "dynamodb:BatchGetItem",
          "dynamodb:BatchWriteItem", "dynamodb:TransactGetItems", "dynamodb:TransactWriteItems"
        ],
        "Resource": "${dynamoDb.table.arn}"
      }]
    }`,
  },
]);

const apiPostShopLambda = createLambda({
  name: 'shop-api-post-shop',
  config,
  role: apiPostShopRole,
  environment: {
    DYNAMODB_TABLE_NAME: dynamoDb.table.name,
  },
  snsTopicArn: alarmTopic.arn,
  createAlarms: false,
});

createApiRoute('ApiPostShop', {
  api: apiGateway.api,
  routeKey: 'POST /api/v1/shops',
  lambda: apiPostShopLambda.lambda,
  accountId: accountId,
  region: awsRegion,
  routePattern: '/api/v1/shops',
});

// ============================================================================
// User API Handlers
// ============================================================================

// API Get User Account
const apiGetUserAccountRole = createLambdaRole('api-user-get-account', stageName, [
  dynamoDBReadPolicy(dynamoDb.table.arn),
]);

const apiGetUserAccountLambda = createLambda({
  name: 'user-api-get-account',
  config,
  role: apiGetUserAccountRole,
  environment: {
    DYNAMODB_TABLE_NAME: dynamoDb.table.name,
  },
  snsTopicArn: alarmTopic.arn,
  createAlarms: false,
});

createApiRoute('ApiGetUserAccount', {
  api: apiGateway.api,
  routeKey: 'GET /api/v1/me/account',
  lambda: apiGetUserAccountLambda.lambda,
  authorizerId: apiGateway.authorizer.id,
  accountId: accountId,
  region: awsRegion,
  routePattern: '/api/v1/me/account*',
});

// API Patch User Account
const apiPatchUserAccountRole = createLambdaRole('api-patch-user-account', stageName, [
  {
    name: 'DynamoDBAccess',
    policy: pulumi.interpolate`{
      "Version": "2012-10-17",
      "Statement": [{
        "Effect": "Allow",
        "Action": ["dynamodb:GetItem", "dynamodb:UpdateItem"],
        "Resource": "${dynamoDb.table.arn}"
      }]
    }`,
  },
]);

const apiPatchUserAccountLambda = createLambda({
  name: 'user-api-patch-account',
  config,
  role: apiPatchUserAccountRole,
  environment: {
    DYNAMODB_TABLE_NAME: dynamoDb.table.name,
  },
  memorySize: 256,
  snsTopicArn: alarmTopic.arn,
  createAlarms: false,
});

createApiRoute('ApiPatchUserAccount', {
  api: apiGateway.api,
  routeKey: 'PATCH /api/v1/me/account',
  lambda: apiPatchUserAccountLambda.lambda,
  authorizerId: apiGateway.authorizer.id,
  accountId: accountId,
  region: awsRegion,
  routePattern: '/api/v1/me/account*',
});

// ============================================================================
// Search Filter API Handlers
// ============================================================================

// API Get Search Filter
const apiGetSearchFilterRole = createLambdaRole('api-get-search-filter', stageName, [
  dynamoDBReadPolicy(dynamoDb.table.arn),
]);

const apiGetSearchFilterLambda = createLambda({
  name: 'search-filter-api-get-search-filter',
  config,
  role: apiGetSearchFilterRole,
  environment: {
    DYNAMODB_TABLE_NAME: dynamoDb.table.name,
  },
  snsTopicArn: alarmTopic.arn,
  createAlarms: false,
});

createApiRoute('ApiGetSearchFilter', {
  api: apiGateway.api,
  routeKey: 'GET /api/v1/me/search-filters/{id}',
  lambda: apiGetSearchFilterLambda.lambda,
  authorizerId: apiGateway.authorizer.id,
  accountId: accountId,
  region: awsRegion,
  routePattern: '/api/v1/me/search-filters/*',
});

// API Get Search Filters
const apiGetSearchFiltersRole = createLambdaRole('api-get-search-filters', stageName, [
  dynamoDBReadPolicy(dynamoDb.table.arn),
]);

const apiGetSearchFiltersLambda = createLambda({
  name: 'search-filter-api-get-search-filters',
  config,
  role: apiGetSearchFiltersRole,
  environment: {
    DYNAMODB_TABLE_NAME: dynamoDb.table.name,
  },
  snsTopicArn: alarmTopic.arn,
  createAlarms: false,
});

createApiRoute('ApiGetSearchFilters', {
  api: apiGateway.api,
  routeKey: 'GET /api/v1/me/search-filters',
  lambda: apiGetSearchFiltersLambda.lambda,
  authorizerId: apiGateway.authorizer.id,
  accountId: accountId,
  region: awsRegion,
  routePattern: '/api/v1/me/search-filters',
});

// API Post Search Filter
const apiPostSearchFilterRole = createLambdaRole('api-post-search-filter', stageName, [
  {
    name: 'DynamoDBAccess',
    policy: pulumi.interpolate`{
      "Version": "2012-10-17",
      "Statement": [{
        "Effect": "Allow",
        "Action": ["dynamodb:GetItem", "dynamodb:BatchGetItem", "dynamodb:Query", "dynamodb:PutItem"],
        "Resource": "${dynamoDb.table.arn}"
      }]
    }`,
  },
]);

const apiPostSearchFilterLambda = createLambda({
  name: 'search-filter-api-post-search-filter',
  config,
  role: apiPostSearchFilterRole,
  environment: {
    DYNAMODB_TABLE_NAME: dynamoDb.table.name,
  },
  snsTopicArn: alarmTopic.arn,
  createAlarms: false,
});

createApiRoute('ApiPostSearchFilter', {
  api: apiGateway.api,
  routeKey: 'POST /api/v1/me/search-filters',
  lambda: apiPostSearchFilterLambda.lambda,
  authorizerId: apiGateway.authorizer.id,
  accountId: accountId,
  region: awsRegion,
  routePattern: '/api/v1/me/search-filters',
});

// API Patch Search Filter
const apiPatchSearchFilterRole = createLambdaRole('api-patch-search-filter', stageName, [
  {
    name: 'DynamoDBAccess',
    policy: pulumi.interpolate`{
      "Version": "2012-10-17",
      "Statement": [{
        "Effect": "Allow",
        "Action": ["dynamodb:GetItem", "dynamodb:UpdateItem"],
        "Resource": "${dynamoDb.table.arn}"
      }]
    }`,
  },
]);

const apiPatchSearchFilterLambda = createLambda({
  name: 'search-filter-api-patch-search-filter',
  config,
  role: apiPatchSearchFilterRole,
  environment: {
    DYNAMODB_TABLE_NAME: dynamoDb.table.name,
  },
  snsTopicArn: alarmTopic.arn,
  createAlarms: false,
});

createApiRoute('ApiPatchSearchFilter', {
  api: apiGateway.api,
  routeKey: 'PATCH /api/v1/me/search-filters/{id}',
  lambda: apiPatchSearchFilterLambda.lambda,
  authorizerId: apiGateway.authorizer.id,
  accountId: accountId,
  region: awsRegion,
  routePattern: '/api/v1/me/search-filters/*',
});

// API Delete Search Filter
const apiDeleteSearchFilterRole = createLambdaRole('api-delete-search-filter', stageName, [
  {
    name: 'DynamoDBAccess',
    policy: pulumi.interpolate`{
      "Version": "2012-10-17",
      "Statement": [{
        "Effect": "Allow",
        "Action": ["dynamodb:GetItem", "dynamodb:DeleteItem"],
        "Resource": "${dynamoDb.table.arn}"
      }]
    }`,
  },
]);

const apiDeleteSearchFilterLambda = createLambda({
  name: 'search-filter-api-delete-search-filter',
  config,
  role: apiDeleteSearchFilterRole,
  environment: {
    DYNAMODB_TABLE_NAME: dynamoDb.table.name,
  },
  snsTopicArn: alarmTopic.arn,
  createAlarms: false,
});

createApiRoute('ApiDeleteSearchFilter', {
  api: apiGateway.api,
  routeKey: 'DELETE /api/v1/me/search-filters/{id}',
  lambda: apiDeleteSearchFilterLambda.lambda,
  authorizerId: apiGateway.authorizer.id,
  accountId: accountId,
  region: awsRegion,
  routePattern: '/api/v1/me/search-filters/*',
});

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

// Queue URLs for outputs
export const sendMailQueueUrl = sendMailQueues.queue.url;
export const sendMailDeadLetterQueueUrl = sendMailQueues.dlq.url;
export const productIngestEventsDynamodbQueueUrl = productIngestQueues.queue.url;
export const productIngestEventsDynamodbDeadLetterQueueUrl = productIngestQueues.dlq.url;
export const productMaterializeDynamodbNewQueueUrl = productMaterializeDynamoDbNewQueues.queue.url;
export const productMaterializeDynamodbNewDeadLetterQueueUrl = productMaterializeDynamoDbNewQueues.dlq.url;
export const productMaterializeDynamodbUpdateQueueUrl = productMaterializeDynamoDbUpdateQueues.queue.url;
export const productMaterializeDynamodbUpdateDeadLetterQueueUrl = productMaterializeDynamoDbUpdateQueues.dlq.url;
export const productMaterializeOpensearchNewQueueUrl = productMaterializeOpenSearchNewQueues.queue.url;
export const productMaterializeOpensearchNewDeadLetterQueueUrl = productMaterializeOpenSearchNewQueues.dlq.url;
export const productMaterializeOpensearchUpdateQueueUrl = productMaterializeOpenSearchUpdateQueues.queue.url;
export const productMaterializeOpensearchUpdateDeadLetterQueueUrl = productMaterializeOpenSearchUpdateQueues.dlq.url;
export const productUpdateNotifyUserQueueUrl = productUpdateNotifyUserQueues.queue.url;
export const productUpdateNotifyUserDeadLetterQueueUrl = productUpdateNotifyUserQueues.dlq.url;
export const productEnrichmentQueueUrl = productEnrichmentQueues.queue.url;
export const productEnrichmentDeadLetterQueueUrl = productEnrichmentQueues.dlq.url;
