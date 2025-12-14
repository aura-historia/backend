/**
 * Module for creating SQS queues with Dead Letter Queue pattern
 */

import * as aws from '@pulumi/aws';
import * as pulumi from '@pulumi/pulumi';
import { createDlqAlarm } from './alarms';

export interface QueueConfig {
  name: string;
  stageName: string;
  visibilityTimeoutSeconds?: number;
  maxReceiveCount?: number;
  messageRetentionSeconds?: number;
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
    messageRetentionSeconds: config.messageRetentionSeconds ?? 1209600, // 14 days
  });

  const queue = new aws.sqs.Queue(`${config.name}Q`, {
    name: `${config.name}-queue-${config.stageName}`,
    redrivePolicy: pulumi
      .all([dlq.arn, config.maxReceiveCount ?? 3])
      .apply(([arn, count]) =>
        JSON.stringify({
          deadLetterTargetArn: arn,
          maxReceiveCount: count,
        })
      ),
    visibilityTimeoutSeconds: config.visibilityTimeoutSeconds ?? 360,
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
