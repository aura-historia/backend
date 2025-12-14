/**
 * Module for creating IAM roles with common patterns
 */

import * as aws from '@pulumi/aws';
import * as pulumi from '@pulumi/pulumi';

const lambdaAssumeRolePolicy = JSON.stringify({
  Version: '2012-10-17',
  Statement: [
    {
      Effect: 'Allow',
      Principal: {
        Service: 'lambda.amazonaws.com',
      },
      Action: 'sts:AssumeRole',
    },
  ],
});

export function createLambdaRole(
  name: string,
  stageName: string,
  additionalPolicies?: aws.iam.RoleInlinePolicy[]
): aws.iam.Role {
  return new aws.iam.Role(name, {
    name: `${name}-role-${stageName}`,
    assumeRolePolicy: lambdaAssumeRolePolicy,
    managedPolicyArns: ['arn:aws:iam::aws:policy/service-role/AWSLambdaBasicExecutionRole'],
    inlinePolicies: additionalPolicies,
  });
}

export function dynamoDBReadPolicy(tableArn: pulumi.Output<string>): aws.iam.RoleInlinePolicy {
  return {
    name: 'DynamoDBAccess',
    policy: pulumi.interpolate`{
      "Version": "2012-10-17",
      "Statement": [{
        "Effect": "Allow",
        "Action": [
          "dynamodb:GetItem",
          "dynamodb:BatchGetItem",
          "dynamodb:Query"
        ],
        "Resource": ["${tableArn}", "${tableArn}/index/*"]
      }]
    }`,
  };
}

export function dynamoDBWritePolicy(tableArn: pulumi.Output<string>): aws.iam.RoleInlinePolicy {
  return {
    name: 'DynamoDBAccess',
    policy: pulumi.interpolate`{
      "Version": "2012-10-17",
      "Statement": [{
        "Effect": "Allow",
        "Action": [
          "dynamodb:PutItem",
          "dynamodb:UpdateItem",
          "dynamodb:DeleteItem",
          "dynamodb:GetItem",
          "dynamodb:BatchGetItem",
          "dynamodb:Query"
        ],
        "Resource": "${tableArn}"
      }]
    }`,
  };
}

export function dynamoDBFullAccessPolicy(tableArn: pulumi.Output<string>): aws.iam.RoleInlinePolicy {
  return {
    name: 'DynamoDBAccess',
    policy: pulumi.interpolate`{
      "Version": "2012-10-17",
      "Statement": [{
        "Effect": "Allow",
        "Action": ["dynamodb:*"],
        "Resource": "${tableArn}"
      }]
    }`,
  };
}

export function sqsPollerPolicy(queueArn: pulumi.Output<string>): aws.iam.RoleInlinePolicy {
  return {
    name: 'SQSPollerAccess',
    policy: pulumi.interpolate`{
      "Version": "2012-10-17",
      "Statement": [{
        "Effect": "Allow",
        "Action": [
          "sqs:ReceiveMessage",
          "sqs:DeleteMessage",
          "sqs:GetQueueAttributes",
          "sqs:GetQueueUrl"
        ],
        "Resource": "${queueArn}"
      }]
    }`,
  };
}

export function sqsSendMessagePolicy(queueArn: pulumi.Output<string>): aws.iam.RoleInlinePolicy {
  return {
    name: 'SQSSendMessage',
    policy: pulumi.interpolate`{
      "Version": "2012-10-17",
      "Statement": [{
        "Effect": "Allow",
        "Action": ["sqs:SendMessage"],
        "Resource": "${queueArn}"
      }]
    }`,
  };
}

export function openSearchReadPolicy(domainArn: pulumi.Output<string>, region: string, stageName: string): aws.iam.RoleInlinePolicy {
  return {
    name: 'OpenSearchReadOnly',
    policy: pulumi.interpolate`{
      "Version": "2012-10-17",
      "Statement": [{
        "Effect": "Allow",
        "Action": [
          "opensearch:Describe*",
          "opensearch:List*",
          "opensearch:ESHttpGet",
          "opensearch:ESHttpHead",
          "opensearch:ESHttpPost"
        ],
        "Resource": "${domainArn}/*"
      }]
    }`,
  };
}

export function openSearchFullAccessPolicy(domainArn: pulumi.Output<string>, region: string, stageName: string): aws.iam.RoleInlinePolicy {
  return {
    name: 'OpenSearchAccess',
    policy: pulumi.interpolate`{
      "Version": "2012-10-17",
      "Statement": [{
        "Effect": "Allow",
        "Action": ["opensearch:*"],
        "Resource": "${domainArn}/*"
      }]
    }`,
  };
}

export function sesSendEmailPolicy(): aws.iam.RoleInlinePolicy {
  return {
    name: 'SESAccess',
    policy: JSON.stringify({
      Version: '2012-10-17',
      Statement: [
        {
          Effect: 'Allow',
          Action: ['ses:SendEmail', 'ses:SendRawEmail'],
          Resource: '*',
        },
      ],
    }),
  };
}

export function s3ReadPolicy(bucketName: string): aws.iam.RoleInlinePolicy {
  return {
    name: 'S3Access',
    policy: JSON.stringify({
      Version: '2012-10-17',
      Statement: [
        {
          Effect: 'Allow',
          Action: ['s3:GetObject'],
          Resource: `arn:aws:s3:::${bucketName}/*`,
        },
      ],
    }),
  };
}
