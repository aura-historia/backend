import * as cdk from "aws-cdk-lib";
import * as events from "aws-cdk-lib/aws-events";
import * as iam from "aws-cdk-lib/aws-iam";
import * as targets from "aws-cdk-lib/aws-events-targets";
import * as lambda from "aws-cdk-lib/aws-lambda";
import * as lambdaEventSources from "aws-cdk-lib/aws-lambda-event-sources";
import * as scheduler from "aws-cdk-lib/aws-scheduler";
import * as s3 from "aws-cdk-lib/aws-s3";
import * as sqs from "aws-cdk-lib/aws-sqs";
import { Construct } from "constructs";
import type { StageConfig } from "../config";
import type { DmsCdc } from "./dms-cdc";
import { lambdaReservedConcurrency, type LambdaFunctions, type LambdaKey } from "./lambdas";
import type { QueueCatalog } from "./queues";
import type { WorkerQueueCatalog } from "./worker-queues";
import type { WorkloadStorage } from "./workload-storage";
import type { WorkerScope } from "../worker-queue-config";

export interface EventingProps {
  readonly config: StageConfig;
  readonly workloadStorage: WorkloadStorage;
  readonly queues: QueueCatalog;
  readonly workerQueues: WorkerQueueCatalog;
  readonly functions: LambdaFunctions;
  readonly productListingOpenSearchVersion: lambda.IVersion;
  readonly productListingNormalizationVersion: lambda.IVersion;
  readonly productListingIngestionVersion: lambda.IVersion;
  readonly productContentAssessmentVersion: lambda.IVersion;
  readonly productEmbeddingVersion: lambda.IVersion;
  readonly productTranslationVersion: lambda.IVersion;
  readonly searchFilterMatchNotificationVersion: lambda.IVersion;
  readonly watchlistNotificationVersion: lambda.IVersion;
  readonly notificationDeliveryVersion: lambda.IVersion;
  readonly marketingConsentSyncVersion: lambda.IVersion;
  readonly userIdentityDeletionVersion: lambda.IVersion;
  readonly geoResolutionVersion: lambda.IVersion;
  readonly listingGeographyVersion: lambda.IVersion;
  readonly listingShippingVersion: lambda.IVersion;
  readonly backendCleanupVersion: lambda.IVersion | undefined;
  readonly cdcRouterVersion: lambda.IVersion | undefined;
  readonly cdcRouterActivation: cdk.CfnCondition;
  readonly dmsCdc?: DmsCdc;
}

const CDC_ROUTER_QUEUE_SCOPES = {
  SEARCH_FILTER_MATCH_NOTIFICATION: "search-filter-match-notification",
  WATCHLIST_NOTIFICATION: "watchlist-notification",
  PRODUCT_LISTING_CONTENT_ASSESSMENT: "product-content-assessment",
  PRODUCT_LISTING_TRANSLATION: "product-translation",
  PRODUCT_LISTING_EMBEDDING: "product-embedding",
  PRODUCT_LISTING_OPENSEARCH: "product-listing-opensearch",
  PRODUCT_LISTING_RAW_NORMALIZATION: "product-listing-normalization",
  NOTIFICATION_DELIVERY: "notification-delivery",
  MARKETING_CONSENT_SYNC: "marketing-consent-sync",
  USER_IDENTITY_DELETION: "user-identity-deletion",
  GEO_RESOLUTION: "geo-resolution",
  LISTING_GEOGRAPHY: "listing-geography",
  LISTING_SHIPPING: "listing-shipping",
} as const satisfies Record<string, WorkerScope>;

export function cdcRouterEventSourceMappingIdExportName(stage: string): string {
  return `aura-historia-cdc-router-event-source-mapping-id-${stage}`;
}

export class Eventing extends Construct {
  readonly stripeEventBus: events.IEventBus;
  readonly shopifyEventBus: events.IEventBus;

  constructor(scope: Construct, id: string, props: EventingProps) {
    super(scope, id);

    this.stripeEventBus = events.EventBus.fromEventBusName(this, "StripeEventBus", props.config.stripeEventBusName);
    this.shopifyEventBus = events.EventBus.fromEventBusName(this, "ShopifyEventBus", props.config.shopifyEventBusName);

    createCloudWatchLogRetentionRule(this, props.functions);
    createPartnerEventRules(
      this,
      this.stripeEventBus,
      this.shopifyEventBus,
      props.functions,
      props.queues,
    );

    if (!props.backendCleanupVersion) {
      throw new Error("Real eventing requires a cleanup Lambda version.");
    }
    createMaintenanceSchedules(this, props.config, props.backendCleanupVersion, props.workloadStorage.maintenanceDlq);

    if (!props.functions.cdcRouter || !props.cdcRouterVersion || !props.dmsCdc || !props.cdcRouterActivation) {
      throw new Error("Real eventing requires the DMS CDC router Lambda version, stream, and activation condition.");
    }
    createDmsCdcRouterEventSource(
      this,
      props.config,
      props.functions.cdcRouter,
      props.cdcRouterVersion,
      props.dmsCdc,
      props.workerQueues,
      props.cdcRouterActivation,
      props.workloadStorage.routerFailureArchive,
    );

    createSqsEventSources(this, props);
  }
}


const SCHEDULE_MAXIMUM_EVENT_AGE_SECONDS = 3_600;
const SCHEDULE_MAXIMUM_RETRY_ATTEMPTS = 3;

export function maintenanceSchedulerDeadLetterQueueName(stage: string): string {
  return `aura-historia-maintenance-scheduler-dlq-${stage}`;
}

export function importMaintenanceSchedulerDeadLetterQueue(
  scope: Construct,
  id: string,
  stage: string,
): sqs.IQueue {
  const queueName = maintenanceSchedulerDeadLetterQueueName(stage);
  return sqs.Queue.fromQueueAttributes(scope, id, {
    queueArn: cdk.Stack.of(scope).formatArn({ service: "sqs", resource: queueName }),
    queueName,
  });
}

function createMaintenanceSchedules(
  scope: Construct,
  config: StageConfig,
  cleanupVersion: lambda.IVersion,
  deadLetterQueue: sqs.IQueue,
): void {
  const fxRateSyncArn = cdk.Stack.of(scope).formatArn({
    service: "lambda",
    resource: "function",
    resourceName: `fxrate-lambda-${config.stage}`,
    arnFormat: cdk.ArnFormat.COLON_RESOURCE_NAME,
  });
  const role = new iam.Role(scope, "MaintenanceSchedulerExecutionRole", {
    assumedBy: new iam.ServicePrincipal("scheduler.amazonaws.com"),
  });
  role.addToPolicy(new iam.PolicyStatement({
    actions: ["lambda:InvokeFunction"],
    resources: [`${cleanupVersion.lambda.functionArn}:*`, fxRateSyncArn],
  }));
  role.addToPolicy(new iam.PolicyStatement({
    actions: ["sqs:SendMessage"],
    resources: [deadLetterQueue.queueArn],
  }));
  new scheduler.CfnSchedule(scope, "ExpiredCredentialCleanupSchedule", {
    name: `aura-historia-expired-credential-cleanup-${config.stage}`,
    scheduleExpression: "cron(0 * * * ? *)",
    scheduleExpressionTimezone: "UTC",
    flexibleTimeWindow: { mode: "OFF" },
    state: "ENABLED",
    target: {
      arn: cleanupVersion.functionArn,
      roleArn: role.roleArn,
      input: JSON.stringify({ schedule: "expired-credential-cleanup" }),
      deadLetterConfig: { arn: deadLetterQueue.queueArn },
      retryPolicy: {
        maximumEventAgeInSeconds: SCHEDULE_MAXIMUM_EVENT_AGE_SECONDS,
        maximumRetryAttempts: SCHEDULE_MAXIMUM_RETRY_ATTEMPTS,
      },
    },
  });

  new scheduler.CfnSchedule(scope, "FxRateRefreshSchedule", {
    name: `aura-historia-fxrate-refresh-${config.stage}`,
    scheduleExpression: "cron(0 6,18 * * ? *)",
    scheduleExpressionTimezone: "UTC",
    flexibleTimeWindow: { mode: "OFF" },
    state: "ENABLED",
    target: {
      arn: fxRateSyncArn,
      roleArn: role.roleArn,
      input: fxRateSchedulerInput(),
      deadLetterConfig: { arn: deadLetterQueue.queueArn },
      retryPolicy: {
        maximumEventAgeInSeconds: SCHEDULE_MAXIMUM_EVENT_AGE_SECONDS,
        maximumRetryAttempts: SCHEDULE_MAXIMUM_RETRY_ATTEMPTS,
      },
    },
  });
}

function fxRateSchedulerInput(): string {
  return JSON.stringify({
    version: "0",
    id: "fxrate:<aws.scheduler.scheduled-time>",
    "detail-type": "Scheduled Event",
    source: "aura-historia.scheduler",
    account: "000000000000",
    time: "<aws.scheduler.scheduled-time>",
    region: "eu-central-1",
    resources: ["<aws.scheduler.schedule-arn>"],
    detail: {},
  });
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
          "X-Shopify-Topic": ["product_listings/add", "product_listings/update", "product_listings/remove"],
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

function createDmsCdcRouterEventSource(
  scope: Construct,
  config: StageConfig,
  router: lambda.Function,
  routerVersion: lambda.IVersion,
  dmsCdc: DmsCdc,
  workerQueues: WorkerQueueCatalog,
  activation: cdk.CfnCondition,
  failureArchive: s3.IBucket,
): void {
  router.addEnvironment("STAGE", config.stage);
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

  const mapping = new lambda.CfnEventSourceMapping(scope, "DmsCdcRouterEventSource", {
    batchSize: 100,
    bisectBatchOnFunctionError: true,
    destinationConfig: {
      onFailure: {
        destination: failureArchive.bucketArn,
      },
    },
    enabled: cdk.Fn.conditionIf(activation.logicalId, true, false) as unknown as boolean,
    eventSourceArn: dmsCdc.stream.streamArn,
    functionName: routerVersion.functionArn,
    functionResponseTypes: ["ReportBatchItemFailures"],
    maximumBatchingWindowInSeconds: 1,
    maximumRecordAgeInSeconds: 3600,
    maximumRetryAttempts: 3,
    metricsConfig: { metrics: ["EventCount"] },
    startingPosition: lambda.StartingPosition.TRIM_HORIZON,
  });
  new cdk.CfnOutput(scope, "DmsCdcRouterEventSourceMappingId", {
    value: mapping.attrId,
    exportName: cdcRouterEventSourceMappingIdExportName(config.stage),
  });
}

function createSqsEventSources(
  scope: Construct,
  props: EventingProps,
): void {
  const {
    functions, queues, workerQueues, productListingOpenSearchVersion,
    productListingNormalizationVersion, productListingIngestionVersion,
    productContentAssessmentVersion, productEmbeddingVersion, productTranslationVersion,
    searchFilterMatchNotificationVersion, watchlistNotificationVersion,
    notificationDeliveryVersion, marketingConsentSyncVersion, userIdentityDeletionVersion,
    geoResolutionVersion, listingGeographyVersion, listingShippingVersion,
  } = props;
  const maximumConcurrency = (key: LambdaKey) => lambdaReservedConcurrency(key, props.config);
  functions.shopify.addEventSource(new lambdaEventSources.SqsEventSource(queues.shopify.queue, {
    batchSize: 10,
    reportBatchItemFailures: true,
    maxBatchingWindow: cdk.Duration.seconds(1),
    maxConcurrency: maximumConcurrency("shopify"),
  }));

  functions.productListingIngestion.addToRolePolicy(new iam.PolicyStatement({
    actions: ["sqs:ReceiveMessage", "sqs:DeleteMessage", "sqs:ChangeMessageVisibility", "sqs:GetQueueAttributes"],
    resources: [queues.productListingIngestion.queue.queueArn],
  }));
  grantFailureCustody(functions.productListingIngestion, queues.productListingIngestion.deadLetterQueue);
  const ingestionMapping = new lambda.CfnEventSourceMapping(scope, "ProductListingIngestionQueueEventSource", {
    batchSize: 10,
    enabled: true,
    eventSourceArn: queues.productListingIngestion.queue.queueArn,
    functionName: productListingIngestionVersion.functionArn,
    functionResponseTypes: ["ReportBatchItemFailures"],
    scalingConfig: sqsScalingConfig(maximumConcurrency("productListingIngestion")),
  });
  const consumerPolicy = functions.productListingIngestion.role?.node.tryFindChild("DefaultPolicy") as iam.Policy | undefined;
  if (!consumerPolicy) {
    throw new Error("Ingestion mapping requires the consumer execution policy.");
  }
  ingestionMapping.addResourceDependency(consumerPolicy.node.defaultChild as iam.CfnPolicy);
  // First creation must establish durable consumption before creating producer functions.
  for (const producer of [functions.auraHistoriaApi, functions.shopify]) {
    (producer.node.defaultChild as lambda.CfnFunction).addResourceDependency(ingestionMapping);
  }

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
    enabled: true,
    eventSourceArn: productListingOpenSearch.queue.queueArn,
    functionName: productListingOpenSearchVersion.functionArn,
    scalingConfig: sqsScalingConfig(maximumConcurrency("productListingOpenSearch")),
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
    enabled: true,
    eventSourceArn: productListingNormalization.queue.queueArn,
    functionName: productListingNormalizationVersion.functionArn,
    scalingConfig: sqsScalingConfig(maximumConcurrency("productListingNormalization")),
    functionResponseTypes: ["ReportBatchItemFailures"],
  });

  addWorkerLambdaEventSource(
    scope,
    "ProductContentAssessmentQueueEventSource",
    functions.productContentAssessment,
    workerQueues["product-content-assessment"],
    productContentAssessmentVersion,

    "ProductListing content assessment",
    { maximumConcurrency: maximumConcurrency("productContentAssessment") },
  );
  addWorkerLambdaEventSource(
    scope,
    "ProductEmbeddingQueueEventSource",
    functions.productEmbedding,
    workerQueues["product-embedding"],
    productEmbeddingVersion,

    "ProductListing embedding",
    { maximumConcurrency: maximumConcurrency("productEmbedding") },
  );
  addWorkerLambdaEventSource(
    scope,
    "ProductTranslationQueueEventSource",
    functions.productTranslation,
    workerQueues["product-translation"],
    productTranslationVersion,

    "ProductListing translation",
    { maximumConcurrency: maximumConcurrency("productTranslation") },
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
    enabled: true,
    eventSourceArn: notificationDelivery.queue.queueArn,
    functionName: notificationDeliveryVersion.functionArn,
    scalingConfig: sqsScalingConfig(maximumConcurrency("notificationDelivery")),
    functionResponseTypes: ["ReportBatchItemFailures"],
  });

  addWorkerLambdaEventSource(
    scope,
    "SearchFilterMatchNotificationQueueEventSource",
    functions.searchFilterMatchNotification,
    workerQueues["search-filter-match-notification"],
    searchFilterMatchNotificationVersion,

    "Search-filter match notification",
    { maximumConcurrency: maximumConcurrency("searchFilterMatchNotification") },
  );
  addWorkerLambdaEventSource(
    scope,
    "WatchlistNotificationQueueEventSource",
    functions.watchlistNotification,
    workerQueues["watchlist-notification"],
    watchlistNotificationVersion,

    "Watchlist notification",
    { maximumConcurrency: maximumConcurrency("watchlistNotification") },
  );

  const marketingConsentSync = workerQueues["marketing-consent-sync"];
  addWorkerLambdaEventSource(
    scope,
    "MarketingConsentSyncQueueEventSource",
    functions.marketingConsentSync,
    marketingConsentSync,
    marketingConsentSyncVersion,
    "Marketing consent sync",
    { includeGetQueueUrl: false, dependOnAccessPolicy: true, maximumConcurrency: maximumConcurrency("marketingConsentSync") },
  );
  // addWorkerLambdaEventSource has already rejected a missing queue pair.
  grantFailureCustody(functions.marketingConsentSync, marketingConsentSync!.deadLetterQueue);
  addWorkerLambdaEventSource(
    scope,
    "UserIdentityDeletionQueueEventSource",
    functions.userIdentityDeletion,
    workerQueues["user-identity-deletion"],
    userIdentityDeletionVersion,
    "User identity deletion",
    { includeGetQueueUrl: false, dependOnAccessPolicy: true, maximumConcurrency: maximumConcurrency("userIdentityDeletion") },
  );
  addWorkerLambdaEventSource(
    scope,
    "ListingGeographyQueueEventSource",
    functions.listingGeography,
    workerQueues["listing-geography"],
    listingGeographyVersion,
    "Listing geography",
    { includeGetQueueUrl: false, dependOnAccessPolicy: true, maximumConcurrency: maximumConcurrency("listingGeography") },
  );
  addWorkerLambdaEventSource(
    scope,
    "ListingShippingQueueEventSource",
    functions.listingShipping,
    workerQueues["listing-shipping"],
    listingShippingVersion,
    "Listing shipping",
    { includeGetQueueUrl: false, dependOnAccessPolicy: true, maximumConcurrency: maximumConcurrency("listingShipping") },
  );
  addWorkerLambdaEventSource(scope, "GeoResolutionQueueEventSource", functions.geoResolution,
    workerQueues["geo-resolution"], geoResolutionVersion, "Geographic resolution",
    { includeGetQueueUrl: false, dependOnAccessPolicy: true, maximumConcurrency: maximumConcurrency("geoResolution") });
}

/**
 * FIFO consumers move records that can never succeed into their own dead-letter queue, so a
 * poisoned head releases its message group after one retry instead of the whole retry budget.
 * The grant lands in the execution policy that the event source mapping depends on.
 */
function grantFailureCustody(fn: lambda.Function, deadLetterQueue: sqs.IQueue): void {
  fn.addEnvironment("DEAD_LETTER_QUEUE_URL", deadLetterQueue.queueUrl);
  fn.addToRolePolicy(new iam.PolicyStatement({
    actions: ["sqs:SendMessage"],
    resources: [deadLetterQueue.queueArn],
  }));
}

function addWorkerLambdaEventSource(
  scope: Construct,
  id: string,
  fn: lambda.Function,
  workerQueue: WorkerQueueCatalog[WorkerScope] | undefined,
  version: lambda.IVersion,
  description: string,
  options: { readonly includeGetQueueUrl?: boolean; readonly dependOnAccessPolicy?: boolean; readonly maximumConcurrency?: number } = {},
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
      ...(options.includeGetQueueUrl === false ? [] : ["sqs:GetQueueUrl"]),
    ],
    resources: [workerQueue.queue.queueArn],
  }));
  const mapping = new lambda.CfnEventSourceMapping(scope, id, {
    batchSize: 1,
    enabled: true,
    eventSourceArn: workerQueue.queue.queueArn,
    functionName: version.functionArn,
    functionResponseTypes: ["ReportBatchItemFailures"],
    scalingConfig: sqsScalingConfig(options.maximumConcurrency),
  });
  if (options.dependOnAccessPolicy) {
    const executionPolicy = fn.role?.node.tryFindChild("DefaultPolicy") as iam.Policy | undefined;
    const policyResource = executionPolicy?.node.defaultChild as iam.CfnPolicy | undefined;
    if (!policyResource) {
      throw new Error(`${description} worker mapping requires its execution policy.`);
    }
    mapping.addResourceDependency(policyResource);
  }
}

function sqsScalingConfig(maximumConcurrency: number | undefined): lambda.CfnEventSourceMapping.ScalingConfigProperty | undefined {
  return maximumConcurrency === undefined ? undefined : { maximumConcurrency };
}
