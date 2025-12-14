/**
 * Module for creating SQS queues with Dead Letter Queue pattern
 */

import * as aws from '@pulumi/aws';
import * as pulumi from '@pulumi/pulumi';
import { createDlqAlarm } from './alarms';

export interface QueueConfig {
  name: string;
  stageName: string;
  visibilityTimeout?: number;
  maxReceiveCount?: number;
  messageRetentionPeriod?: number;
  snsTopicArn: pulumi.Output<string>;
  createAlarm?: boolean;
}

export interface QueuePair {
  queue: aws.sqs.Queue;
  dlq: aws.sqs.Queue;
  dlqAlarm?: aws.cloudwatch.MetricAlarm;
}

/**
 * Creates an SQS queue with a dead letter queue
 */
export function createQueueWithDlq(config: QueueConfig): QueuePair {
  const dlq = new aws.sqs.Queue(`${config.name}Dlq`, {
    name: `${config.name}-dlq-${config.stageName}`,
    messageRetentionPeriod: config.messageRetentionPeriod ?? 1209600, // 14 days
  });

  const queue = new aws.sqs.Queue(`${config.name}Q`, {
    name: `${config.name}-queue-${config.stageName}`,
    redrivePolicy: pulumi.interpolate`{
      "deadLetterTargetArn": "${dlq.arn}",
      "maxReceiveCount": ${config.maxReceiveCount ?? 3}
    }`,
    visibilityTimeout: config.visibilityTimeout ?? 360,
  });

  let dlqAlarm: aws.cloudwatch.MetricAlarm | undefined;
  if (config.createAlarm !== false) {
    dlqAlarm = createDlqAlarm(config.name, {
      queue: dlq,
      stageName: config.stageName,
      snsTopicArn: config.snsTopicArn,
    });
  }

  return { queue, dlq, dlqAlarm };
}
