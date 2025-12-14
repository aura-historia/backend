/**
 * Module for creating EventBridge resources
 */

import * as aws from '@pulumi/aws';
import * as pulumi from '@pulumi/pulumi';

export interface EventBridgeResources {
  eventBus: aws.cloudbridge.EventBus;
  pipeRole: aws.iam.Role;
  pipe: aws.pipes.Pipe;
}

export function createDynamoDBEventBridge(
  stageName: string,
  tableName: pulumi.Output<string>,
  tableStreamArn: pulumi.Output<string>,
  eventBusArn: pulumi.Output<string>
): EventBridgeResources {
  const eventBus = new aws.cloudbridge.EventBus('DynamoDbEventBus', {
    name: `dynamodb-event-bus-${stageName}`,
  });

  const pipeRole = new aws.iam.Role('PipeRole', {
    name: `dynamodb-to-eventbridge-pipe-role-${stageName}`,
    assumeRolePolicy: JSON.stringify({
      Version: '2012-10-17',
      Statement: [
        {
          Effect: 'Allow',
          Principal: {
            Service: 'pipes.amazonaws.com',
          },
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
              "Resource": "${eventBusArn}"
            }
          ]
        }`,
      },
    ],
  });

  const pipe = new aws.pipes.Pipe('TableOneStreamToEventBusPipe', {
    name: `tableone-stream-to-eventbus-${stageName}`,
    roleArn: pipeRole.arn,
    source: tableStreamArn,
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
    target: eventBusArn,
    targetParameters: {
      eventbridgeEventBusParameters: {
        detailType: 'DynamoDBStreamRecord',
        source: tableName,
      },
    },
  });

  return { eventBus, pipeRole, pipe };
}

export interface EventRuleConfig {
  name: string;
  eventBusName: pulumi.Output<string>;
  eventPattern: any;
  targetArn: pulumi.Output<string>;
}

export function createEventRule(config: EventRuleConfig): aws.cloudwatch.EventRule {
  const rule = new aws.cloudwatch.EventRule(config.name, {
    name: config.name,
    eventBusName: config.eventBusName,
    eventPattern: JSON.stringify(config.eventPattern),
  });

  new aws.cloudwatch.EventTarget(`${config.name}Target`, {
    rule: rule.name,
    eventBusName: config.eventBusName,
    arn: config.targetArn,
  });

  return rule;
}
