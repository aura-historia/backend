import * as cdk from "aws-cdk-lib";
import * as dynamodb from "aws-cdk-lib/aws-dynamodb";
import * as events from "aws-cdk-lib/aws-events";
import * as targets from "aws-cdk-lib/aws-events-targets";
import * as iam from "aws-cdk-lib/aws-iam";
import * as lambdaEventSources from "aws-cdk-lib/aws-lambda-event-sources";
import * as pipes from "aws-cdk-lib/aws-pipes";
import { Construct } from "constructs";
import type { StageConfig } from "../config";
import type { ApplicationParameters } from "../parameters";
import type { LambdaCatalog } from "./lambdas";
import type { QueueCatalog, QueueKey } from "./queues";

export interface EventingProps {
  readonly config: StageConfig;
  readonly parameters: ApplicationParameters;
  readonly table: dynamodb.Table;
  readonly queues: QueueCatalog;
  readonly functions: LambdaCatalog;
}

export class Eventing extends Construct {
  readonly dynamoDbEventBus: events.EventBus;
  readonly stripeEventBus: events.IEventBus;
  readonly shopifyEventBus: events.IEventBus;

  constructor(scope: Construct, id: string, props: EventingProps) {
    super(scope, id);

    this.dynamoDbEventBus = new events.EventBus(this, "DynamoDbEventBus", {
      eventBusName: `dynamodb-event-bus-${props.parameters.stageName}`,
    });

    this.stripeEventBus = props.config.isEphemeral
      ? new events.EventBus(this, "StripeEventBus", {
          eventBusName: `stripe-event-bus-${props.parameters.stageName}`,
        })
      : events.EventBus.fromEventBusName(this, "StripeEventBus", props.parameters.stripeEventBusName);

    this.shopifyEventBus = props.config.isEphemeral
      ? new events.EventBus(this, "ShopifyEventBus", {
          eventBusName: `shopify-event-bus-${props.parameters.stageName}`,
        })
      : events.EventBus.fromEventBusName(this, "ShopifyEventBus", props.parameters.shopifyEventBusName);

    createDynamoDbStreamPipe(this, props.table, this.dynamoDbEventBus, props.parameters.stageName);
    createDynamoDbRules(this, props.table, this.dynamoDbEventBus, props.queues);
    createPartnerEventRules(this, this.stripeEventBus, this.shopifyEventBus, props.functions, props.queues);
    createCloudWatchLogRetentionRule(this, props.functions);
    createSqsEventSources(props.functions, props.queues);

    if (!props.config.isEphemeral && props.functions.fxRateSync) {
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
  addDynamoDbRule(scope, eventBus, "DynamoDbNotificationSendEventRule", table, {
    detail: {
      eventName: ["INSERT"],
      dynamodb: {
        NewImage: {
          external: { BOOL: [true] },
          sk: { S: [{ prefix: "user#notification#origin_event_id#" }] },
        },
      },
    },
    targets: ["notificationSend"],
    queues,
  });

  addDynamoDbRule(scope, eventBus, "DynamoDbProductEventRecordPercolateSearchFilterEventRule", table, {
    detail: {
      eventName: ["INSERT"],
      dynamodb: {
        NewImage: {
          event_type: { S: [{ prefix: "DOMAIN_" }, { prefix: "ENRICHMENT_" }] },
        },
      },
    },
    targets: ["searchFilterPercolateProduct"],
    queues,
  });

  addDynamoDbRule(scope, eventBus, "DynamoDbProductEventRecordUpdatedNotifyUserEventRule", table, {
    detail: {
      eventName: ["INSERT"],
      dynamodb: {
        NewImage: {
          event_type: { S: [{ prefix: "DOMAIN_PRICE_" }, { prefix: "DOMAIN_STATE_" }] },
        },
      },
    },
    targets: ["productUpdateNotifyUser"],
    queues,
  });

  addDynamoDbRule(scope, eventBus, "DynamoDbProductMaterializeOpenSearchEventRule", table, {
    detail: {
      eventName: ["INSERT"],
      dynamodb: {
        NewImage: {
          event_type: { S: [{ prefix: "DOMAIN_" }, { prefix: "ENRICHMENT_" }, { prefix: "POLICY_" }] },
        },
      },
    },
    targets: ["productMaterializeOpenSearch"],
    queues,
  });

  addDynamoDbRule(scope, eventBus, "DynamoDbPutShopRecordEventRule", table, {
    detail: {
      eventName: ["INSERT", "MODIFY"],
      dynamodb: {
        NewImage: {
          sk: { S: ["shop#details"] },
        },
      },
    },
    targets: ["shopOpenSearchIndex"],
    queues,
  });

  addDynamoDbRule(scope, eventBus, "DynamoDbSearchFilterSyncEventRule", table, {
    detail: {
      eventName: ["INSERT", "MODIFY", "REMOVE"],
      dynamodb: {
        $or: [
          { NewImage: { sk: { S: [{ prefix: "search_filter#" }] } } },
          { Keys: { sk: { S: [{ prefix: "search_filter#" }] } } },
        ],
      },
    },
    targets: ["searchFilterOpenSearchSync"],
    queues,
  });

  addDynamoDbRule(scope, eventBus, "DynamoDbUserIndexEventRule", table, {
    detail: {
      eventName: ["INSERT", "MODIFY"],
      dynamodb: {
        NewImage: {
          sk: { S: ["user#details"] },
        },
      },
    },
    targets: ["userOpenSearchIndex", "userTierUpdate"],
    queues,
  });

  addDynamoDbRule(scope, eventBus, "ProductPipelineEmbedTextDynamoDbProductEventRecordCreatedEventRule", table, {
    detail: {
      eventName: ["INSERT"],
      dynamodb: {
        NewImage: {
          event_type: { S: ["DOMAIN_CREATED"] },
        },
      },
    },
    targets: ["productPipelineEmbedText"],
    queues,
  });

  addDynamoDbRule(scope, eventBus, "ProductPipelineTranslateDynamoDbProductEventRecordCreatedEventRule", table, {
    detail: {
      eventName: ["INSERT"],
      dynamodb: {
        NewImage: {
          event_type: { S: ["ENRICHMENT_EMBEDDED"] },
        },
      },
    },
    targets: ["productPipelineTranslate"],
    queues,
  });
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
    rule.addTarget(new targets.SqsQueue(props.queues[targetQueue].queue));
  }
}

function createPartnerEventRules(
  scope: Construct,
  stripeEventBus: events.IEventBus,
  shopifyEventBus: events.IEventBus,
  functions: LambdaCatalog,
  queues: QueueCatalog,
): void {
  new events.Rule(scope, "ShopifyEventRule", {
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

function createCloudWatchLogRetentionRule(scope: Construct, functions: LambdaCatalog): void {
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

function createSqsEventSources(functions: LambdaCatalog, queues: QueueCatalog): void {
  addSqsEventSource(functions.productPartnerIngest, queues.productPartnerIngest.queue, 10, true, 1);
  addSqsEventSource(functions.searchFilterOpenSearchSync, queues.searchFilterOpenSearchSync.queue, 1, false);
  addSqsEventSource(functions.shopOpenSearchIndex, queues.shopOpenSearchIndex.queue, 1, false);
  addSqsEventSource(functions.userOpenSearchIndex, queues.userOpenSearchIndex.queue, 1, false);
  addSqsEventSource(functions.userTierUpdate, queues.userTierUpdate.queue, 1, false);
  addSqsEventSource(functions.notificationSend, queues.notificationSend.queue, 25, true, 1);
  addSqsEventSource(functions.productMaterializeOpenSearch, queues.productMaterializeOpenSearch.queue, 2500, true, 1);
  addSqsEventSource(functions.productPipelineEmbedText, queues.productPipelineEmbedText.queue, 10, true, 1);
  addSqsEventSource(functions.productPipelineTranslate, queues.productPipelineTranslate.queue, 10, true, 1);
  addSqsEventSource(functions.productUpdateNotifyUser, queues.productUpdateNotifyUser.queue, 10, true, 1);
  addSqsEventSource(functions.searchFilterPercolateProduct, queues.searchFilterPercolateProduct.queue, 10, true, 1);
  addSqsEventSource(functions.shopify, queues.shopify.queue, 10, true, 1);
}

function addSqsEventSource(
  fn: cdk.aws_lambda.Function,
  queue: cdk.aws_sqs.Queue,
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
    eventName: ["INSERT"],
    dynamodb: {
      NewImage: {
        pk: { S: [{ prefix: "user#" }] },
        sk: { S: [{ prefix: "user#notification#origin_event_id#" }] },
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
