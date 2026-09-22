import * as cdk from "aws-cdk-lib";
import { Template } from "aws-cdk-lib/assertions";
import { ApplicationEphemeralStack, createApplicationStacks } from "../src/application-stack";

const ROUTER_FUNCTION_NAME = "cdc-router-lambda-prod";

function productionObservabilityTemplate(): Template {
  const app = new cdk.App({ analyticsReporting: false });
  const stacks = createApplicationStacks(app, { stage: "prod" });
  if (!stacks.observability) {
    throw new Error("Production observability stack is required.");
  }
  return Template.fromStack(stacks.observability);
}

test("production alarms the CDC router's Kinesis lag, failure-archive delivery, errors, and throttles through the existing SNS topic", () => {
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

  template.resourceCountIs("AWS::CloudWatch::Dashboard", 0);
  template.resourceCountIs("AWS::CloudFormation::Stack", 0);
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
