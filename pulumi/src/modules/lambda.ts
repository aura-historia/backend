/**
 * Module for creating Lambda functions with common patterns
 */

import * as aws from '@pulumi/aws';
import * as pulumi from '@pulumi/pulumi';
import { createLambdaAlarms } from './alarms';
import { StackConfig } from '../types';

export interface BaseLambdaConfig {
  name: string;
  config: StackConfig;
  role: aws.iam.Role;
  environment?: pulumi.Input<{ [key: string]: pulumi.Input<string> }>;
  memorySize?: number;
  timeout?: number;
  snsTopicArn: pulumi.Output<string>;
  createAlarms?: boolean;
  errorThreshold?: number;
}

export interface LambdaWithAlarms {
  lambda: aws.lambda.Function;
  errorAlarm?: aws.cloudwatch.MetricAlarm;
  throttleAlarm?: aws.cloudwatch.MetricAlarm;
}

/**
 * Creates a Lambda function with standard configuration
 */
export function createLambda(lambdaConfig: BaseLambdaConfig): LambdaWithAlarms {
  const { name, config, role, environment, memorySize, timeout, snsTopicArn, createAlarms, errorThreshold } =
    lambdaConfig;

  const lambda = new aws.lambda.Function(name, {
    name: `${name}-${config.stageName}`,
    runtime: 'provided.al2023',
    handler: 'lib.handler',
    role: role.arn,
    s3Bucket: config.artifactBucket,
    s3Key: `${name}-${config.stageName}-${config.commitSHA}.zip`,
    memorySize: memorySize ?? 512,
    timeout: timeout ?? 10,
    ephemeralStorage: { size: 512 },
    environment: environment ? { variables: environment } : undefined,
  });

  let errorAlarm: aws.cloudwatch.MetricAlarm | undefined;
  let throttleAlarm: aws.cloudwatch.MetricAlarm | undefined;

  if (createAlarms !== false) {
    const alarms = createLambdaAlarms(name, {
      lambda,
      stageName: config.stageName,
      snsTopicArn,
    });
    errorAlarm = alarms.errorAlarm;
    throttleAlarm = alarms.throttleAlarm;

    // Override error threshold if specified
    if (errorThreshold !== undefined) {
      errorAlarm = new aws.cloudwatch.MetricAlarm(`${config.stageName}-${name}-lambda-errors`, {
        name: `${config.stageName}-${name}-lambda-errors`,
        alarmDescription: `Alarm when ${name} Lambda has errors`,
        metricName: 'Errors',
        namespace: 'AWS/Lambda',
        statistic: 'Sum',
        period: 300,
        evaluationPeriods: 1,
        threshold: errorThreshold,
        comparisonOperator: 'GreaterThanOrEqualToThreshold',
        dimensions: {
          FunctionName: lambda.name,
        },
        treatMissingData: 'notBreaching',
        alarmActions: [snsTopicArn],
      });
    }
  }

  return { lambda, errorAlarm, throttleAlarm };
}

/**
 * Creates an event source mapping for Lambda with SQS
 */
export function createSqsEventSourceMapping(
  name: string,
  lambda: aws.lambda.Function,
  queue: aws.sqs.Queue,
  batchSize: number,
  maximumBatchingWindowInSeconds: number
): aws.lambda.EventSourceMapping {
  return new aws.lambda.EventSourceMapping(`${name}Mapping`, {
    functionName: lambda.name,
    eventSourceArn: queue.arn,
    enabled: true,
    batchSize,
    maximumBatchingWindowInSeconds,
    functionResponseTypes: ['ReportBatchItemFailures'],
  });
}
