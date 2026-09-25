import * as cdk from "aws-cdk-lib";
import { Template } from "aws-cdk-lib/assertions";
import { ApplicationEphemeralStack, createApplicationStacks } from "../src/application-stack";

const ROUTER_FUNCTION_NAME = "cdc-router-lambda-prod";
const CONTENT_ASSESSMENT_FUNCTION_NAME = "product-content-assessment-lambda-prod";

function productionObservabilityTemplate(): Template {
  const app = new cdk.App({ analyticsReporting: false });
  const stacks = createApplicationStacks(app, { stage: "prod" });
  if (!stacks.observability) {
    throw new Error("Production observability stack is required.");
  }
  return Template.fromStack(stacks.observability);
}

test("production alarms the CDC router's Kinesis lag, archive transfer, drops, delivery failures, errors, and throttles through the existing SNS topic", () => {
  const template = productionObservabilityTemplate();
  const topicIds = Object.keys(template.findResources("AWS::SNS::Topic"));
  const alarms = Object.values(template.findResources("AWS::CloudWatch::Alarm"));

  expect(topicIds).toHaveLength(1);
  const routerAlarms = alarms.filter((alarm) =>
    JSON.stringify(alarm.Properties.Dimensions).includes(ROUTER_FUNCTION_NAME));
  expect(routerAlarms).toHaveLength(4);

  for (const [suffix, metricName, statistic, threshold] of [
    ["iterator-age", "IteratorAge", "Maximum", 900_000],
    ["destination-delivery-failures", "DestinationDeliveryFailures", "Sum", 1],
    ["errors", "Errors", "Sum", 1],
    ["throttles", "Throttles", "Sum", 1],
  ] as const) {
    template.hasResourceProperties("AWS::CloudWatch::Alarm", {
      AlarmName: `prod-${ROUTER_FUNCTION_NAME}-${suffix}`,
      Namespace: "AWS/Lambda",
      MetricName: metricName,
      Dimensions: [{ Name: "FunctionName", Value: ROUTER_FUNCTION_NAME }],
      Statistic: statistic,
      Period: 300,
      Threshold: threshold,
      EvaluationPeriods: 1,
      ComparisonOperator: "GreaterThanOrEqualToThreshold",
      TreatMissingData: "notBreaching",
      AlarmActions: [{ Ref: topicIds[0] }],
    });
  }

  for (const [alarmName, metricName] of [
    ["prod-cdc-router-archive-delivered", "OnFailureDestinationDeliveredEventCount"],
    ["prod-cdc-router-dropped-event", "DroppedEventCount"],
  ] as const) {
    template.hasResourceProperties("AWS::CloudWatch::Alarm", {
      AlarmName: alarmName,
      Namespace: "AWS/Lambda",
      MetricName: metricName,
      Dimensions: [{ Name: "EventSourceMappingUUID", Value: {
        "Fn::ImportValue": "aura-historia-cdc-router-event-source-mapping-id-prod",
      } }],
      Statistic: "Sum",
      Period: 300,
      Threshold: 1,
      EvaluationPeriods: 1,
      ComparisonOperator: "GreaterThanOrEqualToThreshold",
      TreatMissingData: "notBreaching",
      AlarmActions: [{ Ref: topicIds[0] }],
    });
  }

  template.resourceCountIs("AWS::CloudWatch::Dashboard", 0);
  template.resourceCountIs("AWS::CloudFormation::Stack", 0);
});

test("production alarms maintenance Scheduler DLQ visibility through the existing SNS topic", () => {
  const template = productionObservabilityTemplate();
  const topicIds = Object.keys(template.findResources("AWS::SNS::Topic"));

  template.hasResourceProperties("AWS::CloudWatch::Alarm", {
    AlarmName: "prod-maintenance-scheduler-dlq-visible",
    Namespace: "AWS/SQS",
    MetricName: "ApproximateNumberOfMessagesVisible",
    Dimensions: [{ Name: "QueueName", Value: "aura-historia-maintenance-scheduler-dlq-prod" }],
    Statistic: "Maximum",
    Period: 300,
    Threshold: 1,
    EvaluationPeriods: 1,
    ComparisonOperator: "GreaterThanOrEqualToThreshold",
    TreatMissingData: "notBreaching",
    AlarmActions: [{ Ref: topicIds[0] }],
  });
});

test("production applies the queue-worker error threshold to content assessment", () => {
  const template = productionObservabilityTemplate();
  const topicIds = Object.keys(template.findResources("AWS::SNS::Topic"));

  template.hasResourceProperties("AWS::CloudWatch::Alarm", {
    AlarmName: `prod-${CONTENT_ASSESSMENT_FUNCTION_NAME}-errors`,
    Namespace: "AWS/Lambda",
    MetricName: "Errors",
    Dimensions: [{ Name: "FunctionName", Value: CONTENT_ASSESSMENT_FUNCTION_NAME }],
    Statistic: "Sum",
    Period: 300,
    Threshold: 5,
    EvaluationPeriods: 1,
    ComparisonOperator: "GreaterThanOrEqualToThreshold",
    TreatMissingData: "notBreaching",
    AlarmActions: [{ Ref: topicIds[0] }],
  });
});

test("production alarms DMS source/target lag, DMS and Kinesis capacity, and source WAL storage, with task-state notifications", () => {
  const template = productionObservabilityTemplate();
  const topicIds = Object.keys(template.findResources("AWS::SNS::Topic"));

  for (const [alarmName, namespace, metricName, dimensions, statistic, threshold, evaluationPeriods, comparisonOperator] of [
    [
      "prod-cdc-dms-source-latency",
      "AWS/DMS",
      "CDCLatencySource",
      [{ Name: "ReplicationInstanceIdentifier", Value: "aura-historia-dms-cdc-prod" }, { Name: "ReplicationTaskIdentifier", Value: "aura-historia-cdc-prod" }],
      "Maximum",
      300,
      1,
      "GreaterThanOrEqualToThreshold",
    ],
    [
      "prod-cdc-dms-target-latency",
      "AWS/DMS",
      "CDCLatencyTarget",
      [{ Name: "ReplicationInstanceIdentifier", Value: "aura-historia-dms-cdc-prod" }, { Name: "ReplicationTaskIdentifier", Value: "aura-historia-cdc-prod" }],
      "Maximum",
      300,
      1,
      "GreaterThanOrEqualToThreshold",
    ],
    [
      "prod-cdc-dms-capacity",
      "AWS/DMS",
      "CPUUtilization",
      [{ Name: "ReplicationInstanceIdentifier", Value: "aura-historia-dms-cdc-prod" }],
      "Maximum",
      80,
      3,
      "GreaterThanOrEqualToThreshold",
    ],
    [
      "prod-cdc-kinesis-write-capacity",
      "AWS/Kinesis",
      "WriteProvisionedThroughputExceeded",
      [{ Name: "StreamName", Value: "aura-historia-cdc-prod" }],
      "Maximum",
      1,
      1,
      "GreaterThanOrEqualToThreshold",
    ],
    [
      "prod-cdc-source-wal-storage",
      "AWS/RDS",
      "FreeStorageSpace",
      [{ Name: "DBInstanceIdentifier", Value: "aura-historia-postgres-prod" }],
      "Minimum",
      10 * 1024 * 1024 * 1024,
      1,
      "LessThanOrEqualToThreshold",
    ],
  ] as const) {
    template.hasResourceProperties("AWS::CloudWatch::Alarm", {
      AlarmName: alarmName,
      Namespace: namespace,
      MetricName: metricName,
      Dimensions: dimensions,
      Statistic: statistic,
      Period: 300,
      Threshold: threshold,
      EvaluationPeriods: evaluationPeriods,
      ComparisonOperator: comparisonOperator,
      TreatMissingData: "notBreaching",
      AlarmActions: [{ Ref: topicIds[0] }],
    });
  }

  template.hasResourceProperties("AWS::Events::Rule", {
    EventPattern: {
      source: ["aws.dms"],
      "detail-type": ["DMS Replication Task State Change"],
      detail: { eventType: ["REPLICATION_TASK_FAILED", "REPLICATION_TASK_STOPPED"] },
    },
    Targets: [{ Arn: { Ref: topicIds[0] }, Id: "Target0" }],
  });
});

test("dev and ephemeral do not create router observability alarms", () => {
  const devApp = new cdk.App({ analyticsReporting: false });
  const dev = createApplicationStacks(devApp, { stage: "dev" });

  expect(dev.observability).toBeUndefined();
  for (const stack of [dev.network, dev.data, dev.initialization, dev.compute, dev.api]) {
    if (stack) {
      Template.fromStack(stack).resourceCountIs("AWS::CloudWatch::Alarm", 0);
    }
  }

  const ephemeralApp = new cdk.App({ analyticsReporting: false });
  const ephemeral = Template.fromStack(
    new ApplicationEphemeralStack(ephemeralApp, "application-ephemeral", { stage: "ephemeral" }),
  );
  ephemeral.resourceCountIs("AWS::CloudWatch::Alarm", 0);
});
