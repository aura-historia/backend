import * as cdk from "aws-cdk-lib";
import { Template } from "aws-cdk-lib/assertions";
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { createApplicationStacks } from "../src/application-stack";
import { CONTAINER_LOG_SERVICES, stageConfig } from "../src/config";
import { HOST_METRICS } from "../src/constructs/host-monitoring";

type Resource = { readonly Properties: Record<string, any>; readonly DeletionPolicy?: string };

const config = stageConfig("prod");
const stacks = createApplicationStacks(new cdk.App({ analyticsReporting: false }), {
  stage: "prod",
  env: { account: "123456789012", region: "eu-central-1" },
});
const monitoring = Template.fromStack(stacks.monitoring);
const edge = Template.fromStack(stacks.edgeMonitoring);

function resources(template: Template, type: string): Resource[] {
  return Object.values(template.findResources(type)) as Resource[];
}

function alarm(template: Template, name: string): Resource {
  const found = resources(template, "AWS::CloudWatch::Alarm").filter((resource) => resource.Properties.AlarmName === name);
  expect(found).toHaveLength(1);
  return found[0];
}

function topicId(template: Template): string {
  const ids = Object.keys(template.findResources("AWS::SNS::Topic"));
  expect(ids).toHaveLength(1);
  return ids[0];
}

describe("monitoring stacks", () => {
  test("give each Compose service a 30-day log group under the prefix the host user may write", () => {
    const groups = resources(monitoring, "AWS::Logs::LogGroup");
    expect(groups.map((group) => group.Properties.LogGroupName).sort())
      .toEqual(CONTAINER_LOG_SERVICES.map((service) => `/aura-historia/prod/host/${service}`).sort());
    for (const group of groups) {
      expect(group.Properties.LogGroupName.startsWith(config.host.containerLogGroupPrefix)).toBe(true);
      expect(group.Properties.RetentionInDays).toBe(30);
    }
  });

  test("names every Compose log group exactly once", () => {
    const compose = readFileSync(join(__dirname, "../../host/prod/compose.yaml"), "utf8");
    const named = [...compose.matchAll(/awslogs-group: (\S+)\}/g)].map((match) => match[1]).sort();
    expect(named).toEqual(CONTAINER_LOG_SERVICES.map((service) => `/aura-historia/prod/host/${service}`).sort());
  });

  test("watch the metric names the worker publishes", () => {
    const worker = readFileSync(join(__dirname, "../../src/aura-historia-worker/src/health_metrics.rs"), "utf8");
    for (const name of Object.values(HOST_METRICS)) expect(worker).toContain(`: &str = "${name}";`);
    for (const resource of resources(monitoring, "AWS::CloudWatch::Alarm")) {
      expect(resource.Properties.Namespace).toBe(config.host.metricNamespace);
      expect(Object.values(HOST_METRICS)).toContain(resource.Properties.MetricName);
    }
  });

  test("catch a dead host: a missing heartbeat breaches", () => {
    const heartbeat = alarm(monitoring, "aura-historia-prod-host-heartbeat-missing").Properties;
    expect(heartbeat).toMatchObject({
      MetricName: "Heartbeat", Statistic: "SampleCount", Period: 60, Threshold: 1,
      ComparisonOperator: "LessThanThreshold", EvaluationPeriods: 5, DatapointsToAlarm: 5,
      TreatMissingData: "breaching",
    });
    // Without a recorded backup the backup is as stale as an old one.
    expect(alarm(monitoring, "aura-historia-prod-host-backup-stale").Properties)
      .toMatchObject({ Threshold: 26 * 3600, TreatMissingData: "breaching" });
    for (const name of ["postgres-unreachable", "normal-backlog-age", "bulk-backlog-age", "parked-jobs", "low-free-disk"]) {
      expect(alarm(monitoring, `aura-historia-prod-host-${name}`).Properties.TreatMissingData).toBe("notBreaching");
    }
  });

  test("alarm on each backlog class at its own age", () => {
    for (const [backlogClass, threshold] of [["normal", 900], ["bulk", 43200]] as const) {
      expect(alarm(monitoring, `aura-historia-prod-host-${backlogClass}-backlog-age`).Properties).toMatchObject({
        MetricName: "OldestJobAgeSeconds",
        Dimensions: [{ Name: "Class", Value: backlogClass }],
        Threshold: threshold,
        ComparisonOperator: "GreaterThanOrEqualToThreshold",
      });
    }
    expect(alarm(monitoring, "aura-historia-prod-host-parked-jobs").Properties).toMatchObject({ Threshold: 1 });
    expect(alarm(monitoring, "aura-historia-prod-host-low-free-disk").Properties)
      .toMatchObject({ Statistic: "Minimum", Threshold: 20, ComparisonOperator: "LessThanOrEqualToThreshold" });
  });

  test("alarm on the 5xx share of api. and mcp. in the edge region, ignoring near-idle periods", () => {
    for (const [name, distributionId] of [["api", "EP6DVM7VDGBW3"], ["mcp", "ESDMRH0023HN6"]]) {
      const properties = alarm(edge, `aura-historia-prod-${name}-cloudfront-5xx-rate`).Properties;
      expect(properties).toMatchObject({ Threshold: 5, EvaluationPeriods: 2, DatapointsToAlarm: 2, TreatMissingData: "notBreaching" });
      const queries = properties.Metrics as any[];
      expect(queries.find((query) => query.ReturnData !== false).Expression).toBe("IF(requests >= 10, rate, 0)");
      for (const query of queries.filter((query) => query.MetricStat)) {
        expect(query.MetricStat.Metric.Namespace).toBe("AWS/CloudFront");
        expect(query.MetricStat.Metric.Dimensions).toEqual([
          { Name: "DistributionId", Value: distributionId }, { Name: "Region", Value: "Global" },
        ]);
      }
    }
    expect(resources(edge, "AWS::Logs::LogGroup")).toEqual([]);
  });

  test("email every alarm and its recovery to the operators' unpublished address", () => {
    for (const template of [monitoring, edge]) {
      const topic = topicId(template);
      const alarms = resources(template, "AWS::CloudWatch::Alarm");
      expect(alarms.length).toBeGreaterThan(0);
      for (const resource of alarms) {
        expect(resource.Properties.AlarmActions).toEqual([{ Ref: topic }]);
        expect(resource.Properties.OKActions).toEqual([{ Ref: topic }]);
      }
      const [subscription] = resources(template, "AWS::SNS::Subscription");
      expect(subscription.Properties).toMatchObject({
        Protocol: "email",
        Endpoint: "{{resolve:ssm:/monitoring/prod/alarm-email}}",
        TopicArn: { Ref: topic },
      });
    }
    // CloudFormation resolves dynamic references anywhere, including construct paths in metadata.
    for (const stack of [stacks.monitoring, stacks.edgeMonitoring]) {
      const paths = stack.node.findAll().map((construct) => construct.node.path);
      expect(paths.filter((path) => path.includes("resolve"))).toEqual([]);
    }
    expect(resources(monitoring, "AWS::CloudWatch::Alarm")).toHaveLength(7);
    expect(resources(edge, "AWS::CloudWatch::Alarm")).toHaveLength(2);
  });
});
