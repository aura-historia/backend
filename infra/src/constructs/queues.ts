import * as cdk from "aws-cdk-lib";
import * as sqs from "aws-cdk-lib/aws-sqs";
import { Construct } from "constructs";
import type { StageConfig } from "../config";

export const QUEUE_DEFINITIONS = {







  shopify: {
    id: "ShopifyLambda",
    queueName: "shopify-lambda-queue",
    deadLetterQueueName: "shopify-lambda-dlq",
    visibilityTimeoutSeconds: 180,
    maxReceiveCount: 5,
  },


} as const;

export type QueueKey = keyof typeof QUEUE_DEFINITIONS;

export interface QueuePair {
  readonly queue: sqs.IQueue;
  readonly deadLetterQueue: sqs.IQueue;
}

export type QueueCatalog = Record<QueueKey, QueuePair>;

export interface QueuesProps {
  readonly config: StageConfig;
  readonly stageName: string;
}

export class Queues extends Construct {
  readonly catalog: QueueCatalog;

  constructor(scope: Construct, id: string, props: QueuesProps) {
    super(scope, id);

    const entries = Object.entries(QUEUE_DEFINITIONS).map(([key, definition]) => {
      const deadLetterQueue = new sqs.Queue(this, `${definition.id}DeadLetterQueue`, {
        queueName: stageQueueName(definition.deadLetterQueueName, props.stageName),
        retentionPeriod: cdk.Duration.days(14),
        encryption: hasManagedSse(definition) ? sqs.QueueEncryption.SQS_MANAGED : undefined,
        removalPolicy: props.config.removalPolicy,
      });

      const queue = new sqs.Queue(this, `${definition.id}Queue`, {
        queueName: stageQueueName(definition.queueName, props.stageName),
        visibilityTimeout: cdk.Duration.seconds(definition.visibilityTimeoutSeconds),
        deadLetterQueue: {
          queue: deadLetterQueue,
          maxReceiveCount: definition.maxReceiveCount,
        },
        encryption: hasManagedSse(definition) ? sqs.QueueEncryption.SQS_MANAGED : undefined,
        removalPolicy: props.config.removalPolicy,
      });

      return [key, { queue, deadLetterQueue }];
    });

    this.catalog = Object.fromEntries(entries) as QueueCatalog;
  }
}

export function importQueueCatalog(scope: Construct, id: string, stageName: string): QueueCatalog {
  const importScope = new Construct(scope, id);
  const entries = Object.entries(QUEUE_DEFINITIONS).map(([key, definition]) => {
    const queueName = stageQueueName(definition.queueName, stageName);
    const deadLetterQueueName = stageQueueName(definition.deadLetterQueueName, stageName);

    return [
      key,
      {
        queue: importQueue(importScope, `${definition.id}QueueImport`, queueName),
        deadLetterQueue: importQueue(importScope, `${definition.id}DeadLetterQueueImport`, deadLetterQueueName),
      },
    ];
  });

  return Object.fromEntries(entries) as QueueCatalog;
}

function importQueue(scope: Construct, id: string, queueName: string): sqs.IQueue {
  return sqs.Queue.fromQueueAttributes(scope, id, {
    queueArn: cdk.Stack.of(scope).formatArn({
      service: "sqs",
      resource: queueName,
    }),
    queueName,
    queueUrl: cdk.Fn.sub("https://sqs.${AWS::Region}.${AWS::URLSuffix}/${AWS::AccountId}/${QueueName}", {
      QueueName: queueName,
    }),
  });
}

function stageQueueName(baseName: string, stageName: string): string {
  return `${baseName}-${stageName}`;
}

function hasManagedSse(definition: (typeof QUEUE_DEFINITIONS)[QueueKey]): boolean {
  return "managedSse" in definition && definition.managedSse === true;
}
