import * as cdk from "aws-cdk-lib";
import * as dynamodb from "aws-cdk-lib/aws-dynamodb";
import * as events from "aws-cdk-lib/aws-events";
import * as targets from "aws-cdk-lib/aws-events-targets";
import * as iam from "aws-cdk-lib/aws-iam";
import * as lambda from "aws-cdk-lib/aws-lambda";
import * as lambdaEventSources from "aws-cdk-lib/aws-lambda-event-sources";
import * as fs from "node:fs";
import * as path from "node:path";
import * as pipes from "aws-cdk-lib/aws-pipes";
import * as sqs from "aws-cdk-lib/aws-sqs";
import { Construct } from "constructs";
import type { StageConfig } from "../config";
import type { LambdaFunctions } from "./lambdas";
import type { QueueCatalog, QueueKey } from "./queues";

export interface EventingProps {
  readonly config: StageConfig;
  readonly table: dynamodb.Table;
  readonly queues: QueueCatalog;
  readonly functions: LambdaFunctions;
}

export class Eventing extends Construct {
  readonly dynamoDbEventBus: events.EventBus;
  readonly stripeEventBus: events.IEventBus;
  readonly shopifyEventBus: events.IEventBus;

  constructor(scope: Construct, id: string, props: EventingProps) {
    super(scope, id);

    const stageName = props.config.stage;

    this.dynamoDbEventBus = new events.EventBus(this, "DynamoDbEventBus", {
      eventBusName: `dynamodb-event-bus-${stageName}`,
    });

    this.stripeEventBus = props.config.isEphemeral
      ? new events.EventBus(this, "StripeEventBus", {
          eventBusName: props.config.stripeEventBusName,
        })
      : events.EventBus.fromEventBusName(this, "StripeEventBus", props.config.stripeEventBusName);

    this.shopifyEventBus = props.config.isEphemeral
      ? new events.EventBus(this, "ShopifyEventBus", {
          eventBusName: props.config.shopifyEventBusName,
        })
      : events.EventBus.fromEventBusName(this, "ShopifyEventBus", props.config.shopifyEventBusName);

    createDynamoDbStreamPipe(this, props.table, this.dynamoDbEventBus, stageName);
    createDynamoDbRules(this, props.table, this.dynamoDbEventBus, props.queues);
    createPartnerEventRules(this, this.stripeEventBus, this.shopifyEventBus, props.functions, props.queues);
    createCloudWatchLogRetentionRule(this, props.functions);
    createSqsEventSources(props.functions, props.queues);

    if (!props.config.isEphemeral && props.functions.fxRateSync) {
      createInitialFxRateSnapshot(this, props.functions.fxRateSync, stageName);
      new events.Rule(this, "FxRateSyncStartSchedule", {
        schedule: events.Schedule.expression("cron(0 6,18 * * ? *)"),
        targets: [
          new targets.LambdaFunction(props.functions.fxRateSync, {
            maxEventAge: cdk.Duration.hours(1),
            retryAttempts: 3,
          }),
        ],
      });
    }
  }
}

function createInitialFxRateSnapshot(
  scope: Construct,
  fxRateSync: lambda.IFunction,
  stageName: string,
): void {
  const provider = new lambda.Function(scope, "InitialFxRateSnapshotProvider", {
    functionName: `fxrate-initial-snapshot-provider-${stageName}`,
    runtime: lambda.Runtime.NODEJS_20_X,
    handler: "index.handler",
    timeout: cdk.Duration.seconds(30),
    code: lambda.Code.fromInline(resourceCode("fx-rate-initial-snapshot-custom-resource.js")),
  });
  fxRateSync.grantInvoke(provider);

  new cdk.CustomResource(scope, "InitialFxRateSnapshot", {
    serviceToken: provider.functionArn,
    properties: {
      FunctionName: fxRateSync.functionName,
      SourceEventId: `deployment:fxrate:initial:${stageName}:v1`,
    },
  });
}

function resourceCode(fileName: string): string {
  return fs.readFileSync(path.join(__dirname, "..", "resources", fileName), "utf8");
}

function createDynamoDbStreamPipe(
  scope: Construct,
  table: dynamodb.Table,
  eventBus: events.EventBus,
  stageName: string,
): void {
  const pipeRole = new iam.Role(scope, "TableOneStreamToEventBusPipeRole", {
    assumedBy: new iam.ServicePrincipal("pipes.amazonaws.com"),
  });
  table.grantStreamRead(pipeRole);
  eventBus.grantPutEventsTo(pipeRole);

  new pipes.CfnPipe(scope, "TableOneStreamToEventBusPipe", {
    name: `tableone-stream-to-eventbus-${stageName}`,
    roleArn: pipeRole.roleArn,
    source: table.tableStreamArn!,
    sourceParameters: {
      dynamoDbStreamParameters: {
        batchSize: 10,
        maximumBatchingWindowInSeconds: 1,
        startingPosition: "LATEST",
      },
      filterCriteria: {
        filters: DYNAMODB_STREAM_FILTER_PATTERNS.map((pattern) => ({ pattern })),
      },
    },
    target: eventBus.eventBusArn,
    targetParameters: {
      eventBridgeEventBusParameters: {
        detailType: "DynamoDBStreamRecord",
        source: table.tableName,
      },
    },
  });
}

function createDynamoDbRules(
  scope: Construct,
  table: dynamodb.Table,
  eventBus: events.EventBus,
  queues: QueueCatalog,
): void {


}

function addDynamoDbRule(
  scope: Construct,
  eventBus: events.EventBus,
  id: string,
  table: dynamodb.Table,
  props: {
    readonly detail: Record<string, unknown>;
    readonly targets: readonly QueueKey[];
    readonly queues: QueueCatalog;
  },
): void {
  const rule = new events.Rule(scope, id, {
    eventBus,
    eventPattern: {
      source: [table.tableName],
      detailType: ["DynamoDBStreamRecord"],
      detail: props.detail,
    } as events.EventPattern,
  });

  for (const targetQueue of props.targets) {
    const queue = props.queues[targetQueue].queue;
    rule.addTarget(new targets.SqsQueue(queue));
    allowEventRuleToSendToQueue(scope, `${id}${targetQueue}QueuePolicy`, rule, queue);
  }
}

function createPartnerEventRules(
  scope: Construct,
  stripeEventBus: events.IEventBus,
  shopifyEventBus: events.IEventBus,
  functions: LambdaFunctions,
  queues: QueueCatalog,
): void {
  const shopifyRule = new events.Rule(scope, "ShopifyEventRule", {
    eventBus: shopifyEventBus,
    eventPattern: {
      detail: {
        metadata: {
          "X-Shopify-Topic": ["products/create", "products/update", "products/delete"],
        },
      },
    },
    targets: [new targets.SqsQueue(queues.shopify.queue)],
  });
  allowEventRuleToSendToQueue(scope, "ShopifyEventRuleQueuePolicy", shopifyRule, queues.shopify.queue);

  new events.Rule(scope, "StripeEventRule", {
    eventBus: stripeEventBus,
    eventPattern: {
      detail: {
        type: [
          "customer.subscription.created",
          "customer.subscription.updated",
          "customer.subscription.deleted",
        ],
      },
    },
    targets: [new targets.LambdaFunction(functions.stripe)],
  });
}

function allowEventRuleToSendToQueue(scope: Construct, id: string, rule: events.Rule, queue: sqs.IQueue): void {
  new sqs.CfnQueuePolicy(scope, id, {
    queues: [queue.queueUrl],
    policyDocument: {
      Version: "2012-10-17",
      Statement: [
        {
          Effect: "Allow",
          Principal: {
            Service: "events.amazonaws.com",
          },
          Action: "sqs:SendMessage",
          Resource: queue.queueArn,
          Condition: {
            ArnEquals: {
              "aws:SourceArn": rule.ruleArn,
            },
          },
        },
      ],
    },
  });
}

function createCloudWatchLogRetentionRule(scope: Construct, functions: LambdaFunctions): void {
  new events.Rule(scope, "CloudWatchLogGroupCreatedEventRule", {
    eventPattern: {
      source: ["aws.logs"],
      detailType: ["AWS API Call via CloudTrail"],
      detail: {
        eventSource: ["logs.amazonaws.com"],
        eventName: ["CreateLogGroup"],
      },
    },
    targets: [new targets.LambdaFunction(functions.cloudWatchLogRetention)],
  });
}

function createSqsEventSources(functions: LambdaFunctions, queues: QueueCatalog): void {



  addSqsEventSource(functions.shopify, queues.shopify.queue, 10, true, 1);
}

function addSqsEventSource(
  fn: cdk.aws_lambda.Function,
  queue: sqs.IQueue,
  batchSize: number,
  reportBatchItemFailures: boolean,
  maxBatchingWindowSeconds?: number,
): void {
  fn.addEventSource(
    new lambdaEventSources.SqsEventSource(queue, {
      batchSize,
      reportBatchItemFailures,
      maxBatchingWindow: maxBatchingWindowSeconds === undefined ? undefined : cdk.Duration.seconds(maxBatchingWindowSeconds),
    }),
  );
}

const DYNAMODB_STREAM_FILTER_PATTERNS = [
  JSON.stringify({
    eventName: ["INSERT"],
    dynamodb: {
      NewImage: {
        pk: { S: [{ prefix: "product#shop_id#" }] },
        sk: { S: [{ prefix: "product#event#" }] },
      },
    },
  }),
  JSON.stringify({
    eventName: ["MODIFY"],
    dynamodb: {
      NewImage: {
        pk: { S: [{ prefix: "user#" }] },
        sk: { S: ["user#details"] },
      },
    },
  }),
  JSON.stringify({
    eventName: ["INSERT", "MODIFY"],
    dynamodb: {
      NewImage: {
        pk: { S: [{ prefix: "shop#shop_id#" }] },
        sk: { S: ["shop#details"] },
      },
    },
  }),

  JSON.stringify({
    eventName: ["INSERT", "MODIFY", "REMOVE"],
    dynamodb: {
      $or: [
        {
          NewImage: {
            pk: { S: [{ prefix: "user#" }] },
            sk: { S: [{ prefix: "search_filter#" }] },
          },
        },
        {
          Keys: {
            pk: { S: [{ prefix: "user#" }] },
            sk: { S: [{ prefix: "search_filter#" }] },
          },
        },
      ],
    },
  }),
];
