import * as cdk from "aws-cdk-lib";
import { Template } from "aws-cdk-lib/assertions";
import { readdirSync, readFileSync } from "node:fs";
import { join } from "node:path";
import { createApplicationStacks } from "../src/application-stack";
import { CONTAINER_LOG_SERVICES, stageConfig } from "../src/config";
import { CRAWLER_METRICS, HOST_METRICS, INGESTION_REJECTED_METRIC } from "../src/constructs/host-monitoring";

type Resource = { readonly Properties: Record<string, any>; readonly DeletionPolicy?: string };

const config = stageConfig("prod");
const containerLogGroups = CONTAINER_LOG_SERVICES.map((service) => `/aura-historia/prod/host/${service}`);
const crawler = config.monitoring.crawler;
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

function alarmsIn(template: Template, namespace: string): Resource[] {
  return resources(template, "AWS::CloudWatch::Alarm").filter((resource) => {
    const metrics = resource.Properties.Metrics
      ? (resource.Properties.Metrics as any[]).filter((query) => query.MetricStat).map((query) => query.MetricStat.Metric)
      : [resource.Properties];
    return metrics.every((watched) => watched.Namespace === namespace);
  });
}

function topicId(template: Template): string {
  const ids = Object.keys(template.findResources("AWS::SNS::Topic"));
  expect(ids).toHaveLength(1);
  return ids[0];
}

describe("monitoring stacks", () => {
  test("give each Compose service a 30-day and the crawler a 7-day log group, under the prefix the host user may write", () => {
    const groups = resources(monitoring, "AWS::Logs::LogGroup");
    expect(groups.map((group) => group.Properties.LogGroupName).sort())
      .toEqual([...containerLogGroups, "/aura-historia/prod/host/crawler"].sort());
    for (const group of groups) {
      expect(group.Properties.LogGroupName.startsWith(config.host.containerLogGroupPrefix)).toBe(true);
      expect(group.Properties.RetentionInDays).toBe(group.Properties.LogGroupName === crawler.logGroupName ? 7 : 30);
    }
  });

  test("names every Compose log group exactly once, the crawler's for its log shipper", () => {
    const compose = readFileSync(join(__dirname, "../../host/prod/compose.yaml"), "utf8");
    const named = [...compose.matchAll(/awslogs-group: (\S+)\}/g)].map((match) => match[1]).sort();
    expect(named).toEqual([...containerLogGroups, crawler.logGroupName].sort());
  });

  test("ship the journal of the crawler's units to the crawler's group as unchanged lines CloudWatch reads metrics from", () => {
    const shipper = readFileSync(join(__dirname, "../../host/prod/crawler-logs/fluent-bit.yaml"), "utf8");
    expect(shipper).toContain(`log_group_name: ${crawler.logGroupName}\n`);
    expect(shipper).toContain("auto_create_group: off\n");
    expect(shipper).toContain("log_key: MESSAGE\n");
    expect(shipper).toContain("log_format: json/emf\n");
    const shipped = [...shipper.matchAll(/- _SYSTEMD_UNIT=(\S+)\.service$/gm)].map((match) => match[1]).sort();
    expect(shipped).toEqual(["crawler-server", "review-agent-runner"]);
    for (const unit of shipped) {
      expect(readFileSync(join(__dirname, `../../src/crawler/scripts/linux/${unit}.service`), "utf8")).toContain("[Service]");
    }
  });

  test("watch the metric names the worker publishes", () => {
    const worker = readFileSync(join(__dirname, "../../src/aura-historia-worker/src/health_metrics.rs"), "utf8");
    for (const name of Object.values(HOST_METRICS)) expect(worker).toContain(`: &str = "${name}";`);
    const hostAlarms = alarmsIn(monitoring, config.host.metricNamespace);
    expect(hostAlarms).toHaveLength(13);
    for (const resource of hostAlarms) {
      expect(resource.Properties.AlarmName.startsWith("aura-historia-prod-host-")).toBe(true);
      const metrics = resource.Properties.Metrics
        ? (resource.Properties.Metrics as any[]).filter((query) => query.MetricStat).map((query) => query.MetricStat.Metric)
        : [resource.Properties];
      for (const watched of metrics) {
        expect([...Object.values(HOST_METRICS), INGESTION_REJECTED_METRIC]).toContain(watched.MetricName);
      }
    }
  });

  test("alarm on the crawler metrics its metric records publish, as the crawler runbook requires", () => {
    const records = readFileSync(join(__dirname, "../../src/crawler/src/metrics.rs"), "utf8");
    expect(records).toContain(`METRICS_NAMESPACE: &str = "${crawler.metricNamespace}";`);
    for (const name of Object.values(CRAWLER_METRICS)) expect(records).toMatch(new RegExp(`^\\s+${name} => \\w+,$`, "m"));
    expect(alarmsIn(monitoring, crawler.metricNamespace)).toHaveLength(3);
    // A pass records its duration and capture count even when it finds no work, so silence is a stall.
    expect(alarm(monitoring, "aura-historia-prod-crawler-scraper-pass-stalled").Properties).toMatchObject({
      MetricName: "ScraperPassDuration", Statistic: "Maximum", Period: 900, Threshold: 900000,
      ComparisonOperator: "GreaterThanThreshold", EvaluationPeriods: 1, TreatMissingData: "breaching",
    });
    expect(alarm(monitoring, "aura-historia-prod-crawler-no-captures").Properties).toMatchObject({
      MetricName: "ScrapesSucceeded", Statistic: "Sum", Period: 86400, Threshold: 0,
      ComparisonOperator: "LessThanOrEqualToThreshold", EvaluationPeriods: 1, TreatMissingData: "breaching",
    });
    expect(alarm(monitoring, "aura-historia-prod-crawler-spider-runs-failing").Properties).toMatchObject({
      MetricName: "CrawlRunsFailed", Statistic: "Sum", Period: 3600, Threshold: crawler.spiderRunFailuresPerHour,
      ComparisonOperator: "GreaterThanThreshold", EvaluationPeriods: 1, TreatMissingData: "notBreaching",
    });
  });

  test("alarm on each scheduled ops job its timer runs, counted from the job's success marker", () => {
    const systemd = join(__dirname, "../../host/prod/systemd");
    const timed = readdirSync(systemd)
      .filter((name) => name.endsWith(".timer"))
      .map((name) => readFileSync(join(systemd, name), "utf8").match(/^Unit=aura-prod-ops@(.+)\.service$/m)?.[1])
      .filter((job): job is string => job !== undefined)
      .sort();
    expect(Object.keys(config.monitoring.maximumJobAgeSeconds).sort()).toEqual(timed);
    const unit = readFileSync(join(systemd, "aura-prod-ops@.service"), "utf8");
    expect(unit).toContain("ExecStartPost=/usr/bin/touch /var/lib/aura-historia/prod/jobs/%i");
    for (const job of timed) {
      expect(alarm(monitoring, `aura-historia-prod-host-${job}-stale`).Properties).toMatchObject({
        MetricName: "SecondsSinceLastSuccess",
        Dimensions: [{ Name: "Job", Value: job }],
        Threshold: config.monitoring.maximumJobAgeSeconds[job],
        TreatMissingData: "breaching",
      });
    }
  });

  test("alarm when more jobs enter custody, not while custody is merely held", () => {
    const parked = alarm(monitoring, "aura-historia-prod-host-parked-jobs").Properties;
    expect(parked).toMatchObject({ Threshold: 1, ComparisonOperator: "GreaterThanOrEqualToThreshold", TreatMissingData: "notBreaching" });
    const queries = parked.Metrics as any[];
    expect(queries.find((query) => query.ReturnData !== false).Expression).toBe("DIFF(parked)");
    expect(queries.find((query) => query.MetricStat).MetricStat).toMatchObject({
      Metric: { MetricName: "ParkedJobs" }, Stat: "Maximum", Period: 300,
    });
  });

  test("count rejected ingestion commands from the line the worker prints for each", () => {
    const scope = readFileSync(join(__dirname, "../../src/aura-historia-worker/src/scopes/product_listing_ingestion.rs"), "utf8");
    expect(scope).toContain(`r#"{"ingestion_outcome":"rejected"}"#`);
    const [filter] = resources(monitoring, "AWS::Logs::MetricFilter");
    expect(filter.Properties).toMatchObject({
      FilterPattern: '{ $.ingestion_outcome = "rejected" }',
      MetricTransformations: [{ MetricNamespace: "AuraHistoria/Host", MetricName: INGESTION_REJECTED_METRIC, MetricValue: "1" }],
    });
    expect(alarm(monitoring, "aura-historia-prod-host-ingestion-rejected").Properties).toMatchObject({
      MetricName: INGESTION_REJECTED_METRIC, Statistic: "Sum", Period: 300, Threshold: 1,
      ComparisonOperator: "GreaterThanOrEqualToThreshold", TreatMissingData: "notBreaching",
    });
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
    for (const name of ["postgres-unreachable", "normal-backlog-age", "bulk-backlog-age", "embedding-backlog-age", "parked-jobs", "ingestion-rejected", "low-free-disk"]) {
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
    expect(alarm(monitoring, "aura-historia-prod-host-low-free-disk").Properties)
      .toMatchObject({ Statistic: "Minimum", Threshold: 20, ComparisonOperator: "LessThanOrEqualToThreshold" });
     expect(alarm(monitoring, "aura-historia-prod-host-embedding-backlog-age").Properties).toMatchObject({
      MetricName: "OldestPendingEmbeddingAgeSeconds", Statistic: "Maximum", Period: 300, Threshold: 36 * 3600,
      ComparisonOperator: "GreaterThanOrEqualToThreshold", TreatMissingData: "notBreaching",
    });
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
    expect(resources(monitoring, "AWS::CloudWatch::Alarm")).toHaveLength(16);
    expect(resources(edge, "AWS::CloudWatch::Alarm")).toHaveLength(2);
  });
});
