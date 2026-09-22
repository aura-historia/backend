import * as cdk from "aws-cdk-lib";
import * as apigwv2 from "aws-cdk-lib/aws-apigatewayv2";
import * as cloudwatch from "aws-cdk-lib/aws-cloudwatch";
import * as actions from "aws-cdk-lib/aws-cloudwatch-actions";
import * as events from "aws-cdk-lib/aws-events";
import * as targets from "aws-cdk-lib/aws-events-targets";
import * as sns from "aws-cdk-lib/aws-sns";
import { Construct } from "constructs";
import type { StageConfig } from "../config";
import { lambdaFunctionName, type LambdaCatalog, type LambdaKey } from "./lambdas";
import { WORKER_QUEUE_DEFINITIONS } from "../worker-queue-config";
import type { WorkerQueueCatalog } from "./worker-queues";

export interface ObservabilityProps {
  readonly config: StageConfig;
  readonly stageName: string;
  readonly api: apigwv2.HttpApi;
  readonly functions: LambdaCatalog;
  readonly workerQueues: WorkerQueueCatalog;
}

export class Observability extends Construct {
  readonly alarmTopic: sns.Topic | undefined;

  constructor(scope: Construct, id: string, props: ObservabilityProps) {
    super(scope, id);

    if (!props.config.enableProductionObservability) {
      return;
    }

    this.alarmTopic = new sns.Topic(this, "AlarmNotificationTopic", {
      topicName: `cloudwatch-alarms-${props.stageName}`,
      displayName: `CloudWatch Alarms for Aura-Historia Backend stage '${props.stageName}'`,
    });
    const alarmAction = new actions.SnsAction(this.alarmTopic);

    const settings = props.config.workerQueues.alarms;
    for (const workerScope of props.config.workerQueues.enabledScopes) {
      const queues = props.workerQueues[workerScope];
      if (!queues) {
        throw new Error(`Missing alarm queues for enabled worker scope '${workerScope}'.`);
      }
      const definition = WORKER_QUEUE_DEFINITIONS[workerScope];
      const metricOptions = { statistic: "Maximum", period: cdk.Duration.seconds(settings.periodSeconds) };
      const alarmOptions = {
        evaluationPeriods: settings.evaluationPeriods,
        comparisonOperator: cloudwatch.ComparisonOperator.GREATER_THAN_OR_EQUAL_TO_THRESHOLD,
        treatMissingData: cloudwatch.TreatMissingData.NOT_BREACHING,
      };
      new cloudwatch.Alarm(this, `Worker${definition.id}SourceAgeAlarm`, {
        ...alarmOptions,
        alarmName: `${props.stageName}-worker-${workerScope}-source-age`,
        alarmDescription: `Oldest ${workerScope} job is at least ${settings.sourceAgeThresholdSeconds}s old; check consumer health and dependencies.`,
        metric: queues.queue.metricApproximateAgeOfOldestMessage(metricOptions),
        threshold: settings.sourceAgeThresholdSeconds,
      }).addAlarmAction(alarmAction);
      new cloudwatch.Alarm(this, `Worker${definition.id}DeadLetterVisibleAlarm`, {
        ...alarmOptions,
        alarmName: `${props.stageName}-worker-${workerScope}-dlq-visible`,
        alarmDescription: `${workerScope} has dead-letter jobs; investigate before operator-controlled replay.`,
        metric: queues.deadLetterQueue.metricApproximateNumberOfMessagesVisible(metricOptions),
        threshold: settings.deadLetterVisibleThreshold,
      }).addAlarmAction(alarmAction);
    }

    apiAlarm(this, props.stageName, "Api4XXErrorAlarm", "4XXError", props.api, 50, 2, "Sum").addAlarmAction(alarmAction);
    apiAlarm(this, props.stageName, "Api5XXErrorAlarm", "5XXError", props.api, 5, 1, "Sum").addAlarmAction(alarmAction);
    apiAlarm(this, props.stageName, "ApiLatencyAlarm", "IntegrationLatency", props.api, 3000, 2, "Average").addAlarmAction(alarmAction);


    for (const [key, fn] of Object.entries(props.functions) as [LambdaKey, unknown][]) {
      if (!fn) {
        continue;
      }

      const functionName = lambdaFunctionName(key, props.config.stage);
      lambdaAlarm(this, props.stageName, `${key}ErrorAlarm`, "Errors", functionName, queueWorkerKeys.has(key) ? 5 : 1)
        .addAlarmAction(alarmAction);

      if (throttleAlarmLambdaKeys.has(key)) {
        lambdaAlarm(this, props.stageName, `${key}ThrottleAlarm`, "Throttles", functionName, 1).addAlarmAction(alarmAction);
      }
    }

    const cdcRouterFunctionName = lambdaFunctionName("cdcRouter", props.config.stage);
    lambdaAlarm(
      this,
      props.stageName,
      "CdcRouterIteratorAgeAlarm",
      "IteratorAge",
      cdcRouterFunctionName,
      CDC_ROUTER_ITERATOR_AGE_THRESHOLD_MILLISECONDS,
      "Maximum",
    ).addAlarmAction(alarmAction);
    lambdaAlarm(
      this,
      props.stageName,
      "CdcRouterDestinationDeliveryFailuresAlarm",
      "DestinationDeliveryFailures",
      cdcRouterFunctionName,
      1,
    ).addAlarmAction(alarmAction);

    cdcDmsAlarms(this, props.stageName, alarmAction);
    cdcDmsTaskStateNotifications(this, props.stageName, this.alarmTopic);
  }
}

function lambdaAlarm(
  scope: Construct,
  stageName: string,
  id: string,
  metricName: string,
  functionName: string,
  threshold: number,
  statistic = "Sum",
): cloudwatch.Alarm {
  return new cloudwatch.Alarm(scope, id, {
    alarmName: `${stageName}-${toKebabCase(functionName)}-${toKebabCase(metricName)}`,
    alarmDescription: `Alarm when ${functionName} has Lambda ${metricName}`,
    metric: new cloudwatch.Metric({
      namespace: "AWS/Lambda",
      metricName,
      dimensionsMap: { FunctionName: functionName },
      statistic,
      period: cdk.Duration.minutes(5),
    }),
    threshold,
    evaluationPeriods: 1,
    comparisonOperator: cloudwatch.ComparisonOperator.GREATER_THAN_OR_EQUAL_TO_THRESHOLD,
    treatMissingData: cloudwatch.TreatMissingData.NOT_BREACHING,
  });
}

function cdcDmsAlarms(scope: Construct, stageName: string, alarmAction: actions.SnsAction): void {
  const replicationInstanceIdentifier = `aura-historia-dms-cdc-${stageName}`;
  const replicationTaskIdentifier = `aura-historia-cdc-${stageName}`;
  const databaseInstanceIdentifier = `aura-historia-postgres-${stageName}`;
  const streamName = `aura-historia-cdc-${stageName}`;
  const dmsTaskDimensions = { ReplicationInstanceIdentifier: replicationInstanceIdentifier, ReplicationTaskIdentifier: replicationTaskIdentifier };

  cdcMetricAlarm(scope, stageName, "CdcDmsSourceLatencyAlarm", "AWS/DMS", "CDCLatencySource", dmsTaskDimensions, 300)
    .addAlarmAction(alarmAction);
  cdcMetricAlarm(scope, stageName, "CdcDmsTargetLatencyAlarm", "AWS/DMS", "CDCLatencyTarget", dmsTaskDimensions, 300)
    .addAlarmAction(alarmAction);
  cdcMetricAlarm(
    scope,
    stageName,
    "CdcDmsCapacityAlarm",
    "AWS/DMS",
    "CPUUtilization",
    { ReplicationInstanceIdentifier: replicationInstanceIdentifier },
    80,
    "Maximum",
    3,
  ).addAlarmAction(alarmAction);
  cdcMetricAlarm(
    scope,
    stageName,
    "CdcKinesisWriteCapacityAlarm",
    "AWS/Kinesis",
    "WriteProvisionedThroughputExceeded",
    { StreamName: streamName },
    1,
  ).addAlarmAction(alarmAction);

  new cloudwatch.Alarm(scope, "CdcSourceWalStorageAlarm", {
    alarmName: `${stageName}-cdc-source-wal-storage`,
    alarmDescription: "RDS free storage is at or below 10 GiB; investigate logical-slot WAL retention before source storage is exhausted.",
    metric: new cloudwatch.Metric({
      namespace: "AWS/RDS",
      metricName: "FreeStorageSpace",
      dimensionsMap: { DBInstanceIdentifier: databaseInstanceIdentifier },
      statistic: "Minimum",
      period: cdk.Duration.minutes(5),
    }),
    threshold: 10 * 1024 * 1024 * 1024,
    evaluationPeriods: 1,
    comparisonOperator: cloudwatch.ComparisonOperator.LESS_THAN_OR_EQUAL_TO_THRESHOLD,
    treatMissingData: cloudwatch.TreatMissingData.NOT_BREACHING,
  }).addAlarmAction(alarmAction);
}

function cdcMetricAlarm(
  scope: Construct,
  stageName: string,
  id: string,
  namespace: string,
  metricName: string,
  dimensionsMap: Record<string, string>,
  threshold: number,
  statistic = "Maximum",
  evaluationPeriods = 1,
): cloudwatch.Alarm {
  return new cloudwatch.Alarm(scope, id, {
    alarmName: `${stageName}-${toKebabCase(id.replace(/Alarm$/, ""))}`,
    alarmDescription: `CDC ${metricName} requires operator investigation.`,
    metric: new cloudwatch.Metric({
      namespace,
      metricName,
      dimensionsMap,
      statistic,
      period: cdk.Duration.minutes(5),
    }),
    threshold,
    evaluationPeriods,
    comparisonOperator: cloudwatch.ComparisonOperator.GREATER_THAN_OR_EQUAL_TO_THRESHOLD,
    treatMissingData: cloudwatch.TreatMissingData.NOT_BREACHING,
  });
}

function cdcDmsTaskStateNotifications(scope: Construct, stageName: string, alarmTopic: sns.ITopic): void {
  new events.Rule(scope, "CdcDmsTaskStateChangeRule", {
    description: `Notify on stopped or failed DMS replication-task state changes in ${stageName}.`,
    eventPattern: {
      source: ["aws.dms"],
      detailType: ["DMS Replication Task State Change"],
      detail: {
        eventType: ["REPLICATION_TASK_FAILED", "REPLICATION_TASK_STOPPED"],
      },
    },
    targets: [new targets.SnsTopic(alarmTopic)],
  });
}

function apiAlarm(
  scope: Construct,
  stageName: string,
  id: string,
  metricName: string,
  api: apigwv2.HttpApi,
  threshold: number,
  evaluationPeriods: number,
  statistic: string,
): cloudwatch.Alarm {
  return new cloudwatch.Alarm(scope, id, {
    alarmName: `${stageName}-api-${toKebabCase(metricName)}`,
    metric: new cloudwatch.Metric({
      namespace: "AWS/ApiGateway",
      metricName,
      dimensionsMap: { ApiId: api.apiId },
      statistic,
      period: cdk.Duration.minutes(5),
    }),
    threshold,
    evaluationPeriods,
    comparisonOperator:
      metricName === "IntegrationLatency"
        ? cloudwatch.ComparisonOperator.GREATER_THAN_THRESHOLD
        : cloudwatch.ComparisonOperator.GREATER_THAN_OR_EQUAL_TO_THRESHOLD,
    treatMissingData: cloudwatch.TreatMissingData.NOT_BREACHING,
  });
}

const throttleAlarmLambdaKeys = new Set<LambdaKey>(["auraHistoriaApi", "cdcRouter"]);

const queueWorkerKeys = new Set<LambdaKey>([
  "shopify",
  "productListingOpenSearch",
  "productListingNormalization",
]);

const CDC_ROUTER_ITERATOR_AGE_THRESHOLD_MILLISECONDS = 900_000;

function toKebabCase(value: string): string {
  return value
    .replace(/([a-z0-9])([A-Z])/g, "$1-$2")
    .replace(/[^a-zA-Z0-9]+/g, "-")
    .replace(/^-|-$/g, "")
    .toLowerCase();
}
