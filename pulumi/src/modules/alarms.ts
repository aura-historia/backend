/**
 * Module for creating CloudWatch alarms with consistent patterns
 */

import * as aws from '@pulumi/aws';
import * as pulumi from '@pulumi/pulumi';

export interface LambdaAlarmConfig {
  lambda: aws.lambda.Function;
  stageName: string;
  snsTopicArn: pulumi.Output<string>;
  treatMissingData?: string;
}

export interface QueueAlarmConfig {
  queue: aws.sqs.Queue;
  stageName: string;
  snsTopicArn: pulumi.Output<string>;
  treatMissingData?: string;
}

/**
 * Creates standard error and throttle alarms for a Lambda function
 */
export function createLambdaAlarms(
  name: string,
  config: LambdaAlarmConfig
): { errorAlarm: aws.cloudwatch.MetricAlarm; throttleAlarm: aws.cloudwatch.MetricAlarm } {
  const errorAlarm = new aws.cloudwatch.MetricAlarm(`${name}ErrorAlarm`, {
    alarmName: `${config.stageName}-${name}-lambda-errors`,
    alarmDescription: `Alarm when ${name} Lambda has errors`,
    metricName: 'Errors',
    namespace: 'AWS/Lambda',
    statistic: 'Sum',
    period: 300,
    evaluationPeriods: 1,
    threshold: 1,
    comparisonOperator: 'GreaterThanOrEqualToThreshold',
    dimensions: {
      FunctionName: config.lambda.name,
    },
    treatMissingData: config.treatMissingData ?? 'notBreaching',
    alarmActions: [config.snsTopicArn],
  });

  const throttleAlarm = new aws.cloudwatch.MetricAlarm(`${name}ThrottleAlarm`, {
    alarmName: `${config.stageName}-${name}-lambda-throttles`,
    alarmDescription: `Alarm when ${name} Lambda is throttled`,
    metricName: 'Throttles',
    namespace: 'AWS/Lambda',
    statistic: 'Sum',
    period: 300,
    evaluationPeriods: 1,
    threshold: 1,
    comparisonOperator: 'GreaterThanOrEqualToThreshold',
    dimensions: {
      FunctionName: config.lambda.name,
    },
    treatMissingData: config.treatMissingData ?? 'notBreaching',
    alarmActions: [config.snsTopicArn],
  });

  return { errorAlarm, throttleAlarm };
}

/**
 * Creates a DLQ alarm for an SQS queue
 */
export function createDlqAlarm(name: string, config: QueueAlarmConfig): aws.cloudwatch.MetricAlarm {
  return new aws.cloudwatch.MetricAlarm(`${name}DlqAlarm`, {
    alarmName: `${config.stageName}-${name}-dlq-messages`,
    alarmDescription: `Alarm when messages appear in ${name} DLQ`,
    metricName: 'ApproximateNumberOfMessagesVisible',
    namespace: 'AWS/SQS',
    statistic: 'Sum',
    period: 300,
    evaluationPeriods: 1,
    threshold: 1,
    comparisonOperator: 'GreaterThanOrEqualToThreshold',
    dimensions: {
      QueueName: config.queue.name,
    },
    treatMissingData: config.treatMissingData ?? 'notBreaching',
    alarmActions: [config.snsTopicArn],
  });
}
