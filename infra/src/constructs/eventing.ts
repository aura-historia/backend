import * as cdk from "aws-cdk-lib";
import * as events from "aws-cdk-lib/aws-events";
import * as iam from "aws-cdk-lib/aws-iam";
import * as targets from "aws-cdk-lib/aws-events-targets";
import * as lambda from "aws-cdk-lib/aws-lambda";
import * as lambdaEventSources from "aws-cdk-lib/aws-lambda-event-sources";
import * as s3 from "aws-cdk-lib/aws-s3";
import * as sqs from "aws-cdk-lib/aws-sqs";
import { Construct } from "constructs";
import type { StageConfig } from "../config";
import type { DmsCdc } from "./dms-cdc";
import type { LambdaFunctions } from "./lambdas";
import type { QueueCatalog } from "./queues";
import type { WorkerQueueCatalog } from "./worker-queues";
import type { WorkerScope } from "../worker-queue-config";

export interface EventingProps {
  readonly config: StageConfig;
  readonly queues: QueueCatalog;
  readonly workerQueues: WorkerQueueCatalog;
  readonly functions: LambdaFunctions;
  readonly productListingOpenSearchVersion: lambda.IVersion;
  readonly productListingNormalizationVersion: lambda.IVersion;
  readonly productContentAssessmentVersion: lambda.IVersion;
  readonly productEmbeddingVersion: lambda.IVersion;
  readonly searchFilterProjectionVersion: lambda.IVersion;
  readonly searchFilterPercolatorVersion: lambda.IVersion;
  readonly searchFilterMatchNotificationVersion: lambda.IVersion;
  readonly watchlistNotificationVersion: lambda.IVersion;
  readonly notificationDeliveryVersion: lambda.IVersion;
  readonly productListingOpenSearchConsumerActivation: cdk.CfnCondition;
  readonly productListingNormalizationConsumerActivation: cdk.CfnCondition;
  readonly productContentAssessmentConsumerActivation: cdk.CfnCondition;
  readonly productEmbeddingConsumerActivation: cdk.CfnCondition;
  readonly searchFilterProjectionConsumerActivation: cdk.CfnCondition;
  readonly searchFilterPercolatorConsumerActivation: cdk.CfnCondition;
  readonly searchFilterMatchNotificationConsumerActivation: cdk.CfnCondition;
  readonly watchlistNotificationConsumerActivation: cdk.CfnCondition;
  readonly notificationDeliveryConsumerActivation: cdk.CfnCondition;
  readonly cdcRouterActivation?: cdk.CfnCondition;
  readonly dmsCdc?: DmsCdc;
}

const CDC_ROUTER_QUEUE_SCOPES = {
  SEARCH_FILTER_PROJECTION: "search-filter-projection",
  SEARCH_FILTER_PERCOLATOR: "search-filter-percolator",
  SEARCH_FILTER_MATCH_NOTIFICATION: "search-filter-match-notification",
  WATCHLIST_NOTIFICATION: "watchlist-notification",
  PRODUCT_LISTING_CONTENT_ASSESSMENT: "product-content-assessment",
  PRODUCT_LISTING_TRANSLATION: "product-translation",
  PRODUCT_LISTING_EMBEDDING: "product-embedding",
  PRODUCT_LISTING_OPENSEARCH: "product-listing-opensearch",
  PRODUCT_LISTING_RAW_NORMALIZATION: "product-listing-normalization",
  NOTIFICATION_DELIVERY: "notification-delivery",
} as const satisfies Record<string, WorkerScope>;

export class Eventing extends Construct {
  readonly stripeEventBus: events.IEventBus;
  readonly shopifyEventBus: events.IEventBus;

  constructor(scope: Construct, id: string, props: EventingProps) {
    super(scope, id);

    const stageName = props.config.stage;

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

    createCloudWatchLogRetentionRule(this, props.functions);
    createPartnerEventRules(
      this,
      this.stripeEventBus,
      this.shopifyEventBus,
      props.functions,
      props.queues,
      props.productListingOpenSearchConsumerActivation,
    );

    if (!props.config.isEphemeral) {
      if (!props.functions.fxRateSync) {
        throw new Error("Real eventing requires the FX Lambda.");
      }
      const fxRateSyncStartSchedule = new events.Rule(this, "FxRateSyncStartSchedule", {
        enabled: false,
        schedule: events.Schedule.expression("cron(0 6,18 * * ? *)"),
        targets: [
          new targets.LambdaFunction(props.functions.fxRateSync, {
            maxEventAge: cdk.Duration.hours(1),
            retryAttempts: 3,
          }),
        ],
      });
      const fxRateSyncStartScheduleResource = fxRateSyncStartSchedule.node.defaultChild as events.CfnRule;
      fxRateSyncStartScheduleResource.state = cdk.Fn.conditionIf(
        props.productListingOpenSearchConsumerActivation.logicalId,
        "ENABLED",
        "DISABLED",
      ) as unknown as string;

      if (!props.functions.cdcRouter || !props.dmsCdc || !props.cdcRouterActivation) {
        throw new Error("Real eventing requires the DMS CDC router Lambda, stream, and activation condition.");
      }
      createDmsCdcRouterEventSource(
        this,
        props.config,
        props.functions.cdcRouter,
        props.dmsCdc,
        props.workerQueues,
        props.cdcRouterActivation,
      );
    }

    createSqsEventSources(
      this,
      props.functions,
      props.queues,
      props.workerQueues,
      props.productListingOpenSearchVersion,
      props.productListingNormalizationVersion,
      props.productContentAssessmentVersion,
      props.productEmbeddingVersion,
      props.searchFilterProjectionVersion,
      props.searchFilterPercolatorVersion,
      props.searchFilterMatchNotificationVersion,
      props.watchlistNotificationVersion,
      props.notificationDeliveryVersion,
      props.productListingOpenSearchConsumerActivation,
      props.productListingNormalizationConsumerActivation,
      props.productContentAssessmentConsumerActivation,
      props.productEmbeddingConsumerActivation,
      props.searchFilterProjectionConsumerActivation,
      props.searchFilterPercolatorConsumerActivation,
      props.searchFilterMatchNotificationConsumerActivation,
      props.watchlistNotificationConsumerActivation,
      props.notificationDeliveryConsumerActivation,
    );
  }
}


function createPartnerEventRules(
  scope: Construct,
  stripeEventBus: events.IEventBus,
  shopifyEventBus: events.IEventBus,
  functions: LambdaFunctions,
  queues: QueueCatalog,
  activation: cdk.CfnCondition,
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
  setRuleState(shopifyRule, activation);
  allowEventRuleToSendToQueue(scope, "ShopifyEventRuleQueuePolicy", shopifyRule, queues.shopify.queue);

  const stripeRule = new events.Rule(scope, "StripeEventRule", {
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
  setRuleState(stripeRule, activation);
}

function setRuleState(rule: events.Rule, activation: cdk.CfnCondition): void {
  const resource = rule.node.defaultChild as events.CfnRule;
  resource.state = cdk.Fn.conditionIf(activation.logicalId, "ENABLED", "DISABLED") as unknown as string;
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

function createDmsCdcRouterEventSource(
  scope: Construct,
  config: StageConfig,
  router: lambda.Function,
  dmsCdc: DmsCdc,
  workerQueues: WorkerQueueCatalog,
  activation: cdk.CfnCondition,
): void {
  const failureArchive = new s3.Bucket(scope, "CdcRouterFailureArchive", {
    bucketName: `aura-historia-cdc-router-failures-${config.stage}`,
    blockPublicAccess: s3.BlockPublicAccess.BLOCK_ALL,
    encryption: s3.BucketEncryption.S3_MANAGED,
    enforceSSL: true,
    lifecycleRules: [{
      enabled: true,
      expiration: cdk.Duration.days(90),
    }],
    removalPolicy: cdk.RemovalPolicy.RETAIN,
  });

  const sourceQueueArns: string[] = [];
  const deadLetterQueueArns: string[] = [];
  for (const [environmentScope, workerScope] of Object.entries(CDC_ROUTER_QUEUE_SCOPES) as [
    keyof typeof CDC_ROUTER_QUEUE_SCOPES,
    WorkerScope,
  ][]) {
    const workerQueue = workerQueues[workerScope];
    if (!workerQueue) {
      throw new Error(`CDC router requires the ${workerScope} worker queue.`);
    }
    router.addEnvironment(`AURA_HISTORIA_ROUTER_QUEUE_URL_${environmentScope}`, workerQueue.queue.queueUrl);
    sourceQueueArns.push(workerQueue.queue.queueArn);
    deadLetterQueueArns.push(workerQueue.deadLetterQueue.queueArn);
  }

  router.addToRolePolicy(new iam.PolicyStatement({
    actions: [
      "kinesis:DescribeStream",
      "kinesis:DescribeStreamSummary",
      "kinesis:GetRecords",
      "kinesis:GetShardIterator",
      "kinesis:ListShards",
    ],
    resources: [dmsCdc.stream.streamArn],
  }));
  router.addToRolePolicy(new iam.PolicyStatement({
    actions: ["sqs:SendMessage", "sqs:GetQueueAttributes"],
    resources: sourceQueueArns,
  }));
  router.addToRolePolicy(new iam.PolicyStatement({
    actions: ["sqs:GetQueueAttributes"],
    resources: deadLetterQueueArns,
  }));
  router.addToRolePolicy(new iam.PolicyStatement({
    actions: ["s3:ListBucket"],
    resources: [failureArchive.bucketArn],
  }));
  router.addToRolePolicy(new iam.PolicyStatement({
    actions: ["s3:PutObject"],
    resources: [failureArchive.arnForObjects("*")],
  }));

  new lambda.CfnEventSourceMapping(scope, "DmsCdcRouterEventSource", {
    batchSize: 100,
    bisectBatchOnFunctionError: true,
    destinationConfig: {
      onFailure: {
        destination: failureArchive.bucketArn,
      },
    },
    enabled: cdk.Fn.conditionIf(activation.logicalId, true, false) as unknown as boolean,
    eventSourceArn: dmsCdc.stream.streamArn,
    functionName: router.functionArn,
    functionResponseTypes: ["ReportBatchItemFailures"],
    maximumBatchingWindowInSeconds: 1,
    maximumRecordAgeInSeconds: 3600,
    maximumRetryAttempts: 3,
    startingPosition: lambda.StartingPosition.TRIM_HORIZON,
  });
}

function createSqsEventSources(
  scope: Construct,
  functions: LambdaFunctions,
  queues: QueueCatalog,
  workerQueues: WorkerQueueCatalog,
  productListingOpenSearchVersion: lambda.IVersion,
  productListingNormalizationVersion: lambda.IVersion,
  productContentAssessmentVersion: lambda.IVersion,
  productEmbeddingVersion: lambda.IVersion,
  searchFilterProjectionVersion: lambda.IVersion,
  searchFilterPercolatorVersion: lambda.IVersion,
  searchFilterMatchNotificationVersion: lambda.IVersion,
  watchlistNotificationVersion: lambda.IVersion,
  notificationDeliveryVersion: lambda.IVersion,
  activation: cdk.CfnCondition,
  normalizationActivation: cdk.CfnCondition,
  productContentAssessmentActivation: cdk.CfnCondition,
  productEmbeddingActivation: cdk.CfnCondition,
  searchFilterProjectionActivation: cdk.CfnCondition,
  percolatorActivation: cdk.CfnCondition,
  searchFilterMatchNotificationActivation: cdk.CfnCondition,
  watchlistNotificationActivation: cdk.CfnCondition,
  notificationDeliveryActivation: cdk.CfnCondition,
): void {
  addSqsEventSource(functions.shopify, queues.shopify.queue, 10, true, 1, activation);

  const productListingOpenSearch = workerQueues["product-listing-opensearch"];
  if (!productListingOpenSearch) {
    throw new Error("ProductListing OpenSearch worker queue is required for its Lambda event source.");
  }
  functions.productListingOpenSearch.addToRolePolicy(new iam.PolicyStatement({
    actions: [
      "sqs:ReceiveMessage",
      "sqs:DeleteMessage",
      "sqs:GetQueueAttributes",
      "sqs:ChangeMessageVisibility",
      "sqs:GetQueueUrl",
    ],
    resources: [productListingOpenSearch.queue.queueArn],
  }));
  new lambda.CfnEventSourceMapping(scope, "ProductListingOpenSearchQueueEventSource", {
    batchSize: 1,
    enabled: cdk.Fn.conditionIf(activation.logicalId, true, false) as unknown as boolean,
    eventSourceArn: productListingOpenSearch.queue.queueArn,
    functionName: productListingOpenSearchVersion.functionArn,
    functionResponseTypes: ["ReportBatchItemFailures"],
  });

  const searchFilterProjection = workerQueues["search-filter-projection"];
  if (!searchFilterProjection) {
    throw new Error("Search-filter projection worker queue is required for its Lambda event source.");
  }
  functions.searchFilterProjection.addToRolePolicy(new iam.PolicyStatement({
    actions: [
      "sqs:ReceiveMessage",
      "sqs:DeleteMessage",
      "sqs:GetQueueAttributes",
      "sqs:ChangeMessageVisibility",
      "sqs:GetQueueUrl",
    ],
    resources: [searchFilterProjection.queue.queueArn],
  }));
  new lambda.CfnEventSourceMapping(scope, "SearchFilterProjectionQueueEventSource", {
    batchSize: 1,
    enabled: cdk.Fn.conditionIf(searchFilterProjectionActivation.logicalId, true, false) as unknown as boolean,
    eventSourceArn: searchFilterProjection.queue.queueArn,
    functionName: searchFilterProjectionVersion.functionArn,
    functionResponseTypes: ["ReportBatchItemFailures"],
  });

  const productListingNormalization = workerQueues["product-listing-normalization"];
  if (!productListingNormalization) {
    throw new Error("ProductListing normalization worker queue is required for its Lambda event source.");
  }
  functions.productListingNormalization.addToRolePolicy(new iam.PolicyStatement({
    actions: [
      "sqs:ReceiveMessage",
      "sqs:DeleteMessage",
      "sqs:GetQueueAttributes",
      "sqs:ChangeMessageVisibility",
      "sqs:GetQueueUrl",
    ],
    resources: [productListingNormalization.queue.queueArn],
  }));
  new lambda.CfnEventSourceMapping(scope, "ProductListingNormalizationQueueEventSource", {
    batchSize: 10,
    enabled: cdk.Fn.conditionIf(normalizationActivation.logicalId, true, false) as unknown as boolean,
    eventSourceArn: productListingNormalization.queue.queueArn,
    functionName: productListingNormalizationVersion.functionArn,
    functionResponseTypes: ["ReportBatchItemFailures"],
  });

  addWorkerLambdaEventSource(
    scope,
    "ProductContentAssessmentQueueEventSource",
    functions.productContentAssessment,
    workerQueues["product-content-assessment"],
    productContentAssessmentVersion,
    productContentAssessmentActivation,
    "ProductListing content assessment",
  );
  addWorkerLambdaEventSource(
    scope,
    "ProductEmbeddingQueueEventSource",
    functions.productEmbedding,
    workerQueues["product-embedding"],
    productEmbeddingVersion,
    productEmbeddingActivation,
    "ProductListing embedding",
  );

  const notificationDelivery = workerQueues["notification-delivery"];
  if (!notificationDelivery) {
    throw new Error("Notification delivery worker queue is required for its Lambda event source.");
  }
  functions.notificationDelivery.addToRolePolicy(new iam.PolicyStatement({
    actions: [
      "sqs:ReceiveMessage",
      "sqs:DeleteMessage",
      "sqs:GetQueueAttributes",
      "sqs:ChangeMessageVisibility",
      "sqs:GetQueueUrl",
    ],
    resources: [notificationDelivery.queue.queueArn],
  }));
  new lambda.CfnEventSourceMapping(scope, "NotificationDeliveryQueueEventSource", {
    batchSize: 1,
    enabled: cdk.Fn.conditionIf(notificationDeliveryActivation.logicalId, true, false) as unknown as boolean,
    eventSourceArn: notificationDelivery.queue.queueArn,
    functionName: notificationDeliveryVersion.functionArn,
    functionResponseTypes: ["ReportBatchItemFailures"],
  });

  const searchFilterPercolator = workerQueues["search-filter-percolator"];
  if (!searchFilterPercolator) {
    throw new Error("Search-filter percolator worker queue is required for its Lambda event source.");
  }
  functions.searchFilterPercolator.addToRolePolicy(new iam.PolicyStatement({
    actions: [
      "sqs:ReceiveMessage",
      "sqs:DeleteMessage",
      "sqs:GetQueueAttributes",
      "sqs:ChangeMessageVisibility",
      "sqs:GetQueueUrl",
    ],
    resources: [searchFilterPercolator.queue.queueArn],
  }));
  new lambda.CfnEventSourceMapping(scope, "SearchFilterPercolatorQueueEventSource", {
    batchSize: 1,
    enabled: cdk.Fn.conditionIf(percolatorActivation.logicalId, true, false) as unknown as boolean,
    eventSourceArn: searchFilterPercolator.queue.queueArn,
    functionName: searchFilterPercolatorVersion.functionArn,
    functionResponseTypes: ["ReportBatchItemFailures"],
  });

  addWorkerLambdaEventSource(
    scope,
    "SearchFilterMatchNotificationQueueEventSource",
    functions.searchFilterMatchNotification,
    workerQueues["search-filter-match-notification"],
    searchFilterMatchNotificationVersion,
    searchFilterMatchNotificationActivation,
    "Search-filter match notification",
  );
  addWorkerLambdaEventSource(
    scope,
    "WatchlistNotificationQueueEventSource",
    functions.watchlistNotification,
    workerQueues["watchlist-notification"],
    watchlistNotificationVersion,
    watchlistNotificationActivation,
    "Watchlist notification",
  );
}

function addWorkerLambdaEventSource(
  scope: Construct,
  id: string,
  fn: lambda.Function,
  workerQueue: WorkerQueueCatalog[WorkerScope] | undefined,
  version: lambda.IVersion,
  activation: cdk.CfnCondition,
  description: string,
): void {
  if (!workerQueue) {
    throw new Error(`${description} worker queue is required for its Lambda event source.`);
  }
  fn.addToRolePolicy(new iam.PolicyStatement({
    actions: [
      "sqs:ReceiveMessage",
      "sqs:DeleteMessage",
      "sqs:GetQueueAttributes",
      "sqs:ChangeMessageVisibility",
      "sqs:GetQueueUrl",
    ],
    resources: [workerQueue.queue.queueArn],
  }));
  new lambda.CfnEventSourceMapping(scope, id, {
    batchSize: 1,
    enabled: cdk.Fn.conditionIf(activation.logicalId, true, false) as unknown as boolean,
    eventSourceArn: workerQueue.queue.queueArn,
    functionName: version.functionArn,
    functionResponseTypes: ["ReportBatchItemFailures"],
  });
}

function addSqsEventSource(
  fn: cdk.aws_lambda.Function,
  queue: sqs.IQueue,
  batchSize: number,
  reportBatchItemFailures: boolean,
  maxBatchingWindowSeconds?: number,
  activation?: cdk.CfnCondition,
): void {
  fn.addEventSource(
    new lambdaEventSources.SqsEventSource(queue, {
      batchSize,
      enabled: activation
        ? cdk.Fn.conditionIf(activation.logicalId, true, false) as unknown as boolean
        : undefined,
      reportBatchItemFailures,
      maxBatchingWindow: maxBatchingWindowSeconds === undefined ? undefined : cdk.Duration.seconds(maxBatchingWindowSeconds),
    }),
  );
}
