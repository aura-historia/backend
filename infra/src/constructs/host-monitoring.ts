import * as cdk from "aws-cdk-lib";
import * as cloudwatch from "aws-cdk-lib/aws-cloudwatch";
import * as actions from "aws-cdk-lib/aws-cloudwatch-actions";
import * as logs from "aws-cdk-lib/aws-logs";
import * as sns from "aws-cdk-lib/aws-sns";
import { Construct } from "constructs";
import { CONTAINER_LOG_SERVICES, ssmValue, type StageConfig } from "../config";

/** The metric names the worker publishes (src/aura-historia-worker/src/health_metrics.rs). */
export const HOST_METRICS = {
  heartbeat: "Heartbeat",
  postgresReachable: "PostgresReachable",
  oldestJobAge: "OldestJobAgeSeconds",
  parkedJobs: "ParkedJobs",
  freeDisk: "FreeDiskPercent",
  sinceLastBackup: "SecondsSinceLastBackup",
  sinceLastJobSuccess: "SecondsSinceLastSuccess",
} as const;

/**
 * The metric counted from the worker's log: the ingestion scope prints one static
 * `{"ingestion_outcome":…}` line per job (src/aura-historia-worker/src/scopes/product_listing_ingestion.rs).
 */
export const INGESTION_REJECTED_METRIC = "IngestionRejectedCommands";

/** The crawler metrics the alarms watch, from its metric records (src/crawler/src/metrics.rs). */
export const CRAWLER_METRICS = {
  scraperPassDuration: "ScraperPassDuration",
  scrapesSucceeded: "ScrapesSucceeded",
  crawlRunsFailed: "CrawlRunsFailed",
} as const;

export interface AlarmTopicProps {
  readonly config: StageConfig;
}

/**
 * The topic an alarm notifies, in its own region, with the operators' email subscription. Every
 * alarm also notifies when it clears.
 */
export class AlarmTopic extends Construct {
  readonly topic: sns.Topic;

  constructor(scope: Construct, id: string, props: AlarmTopicProps) {
    super(scope, id);
    this.topic = new sns.Topic(this, "Topic", {
      topicName: `aura-historia-${props.config.stage}-alarms`,
      displayName: "Aura Historia alarms",
    });
    // A fixed ID: addSubscription names the construct after the endpoint, and CloudFormation
    // would try to resolve the dynamic reference in its path metadata too.
    new sns.Subscription(this, "EmailSubscription", {
      topic: this.topic,
      protocol: sns.SubscriptionProtocol.EMAIL,
      endpoint: ssmValue(props.config.monitoring.alarmEmailParameterName),
    });
  }

  notify(alarm: cloudwatch.Alarm): cloudwatch.Alarm {
    alarm.addAlarmAction(new actions.SnsAction(this.topic));
    alarm.addOkAction(new actions.SnsAction(this.topic));
    return alarm;
  }
}

export interface HostMonitoringProps {
  readonly config: StageConfig;
}

/**
 * The host's container log groups and the alarms on the worker's health metrics. Every metric
 * comes from the host, so a dead host shows as a missing heartbeat, which the heartbeat alarm
 * treats as breaching.
 */
export class HostMonitoring extends Construct {
  readonly alarms: AlarmTopic;
  readonly logGroups: logs.LogGroup[];

  constructor(scope: Construct, id: string, props: HostMonitoringProps) {
    super(scope, id);
    const { config } = props;
    const monitoring = config.monitoring;
    this.alarms = new AlarmTopic(this, "Alarms", { config });

    // Operational logs only: deleting the stack may delete them.
    this.logGroups = CONTAINER_LOG_SERVICES.map((service) => new logs.LogGroup(this, `${toPascalCase(service)}Logs`, {
      logGroupName: `${config.host.containerLogGroupPrefix}${service}`,
      retention: monitoring.containerLogRetentionDays as logs.RetentionDays,
      removalPolicy: cdk.RemovalPolicy.DESTROY,
    }));

    const metric = (metricName: string, statistic: string, period: cdk.Duration, dimensionsMap?: Record<string, string>) =>
      new cloudwatch.Metric({ namespace: config.host.metricNamespace, metricName, statistic, period, dimensionsMap });
    const alarm = (name: string, description: string, alarmProps: Omit<cloudwatch.AlarmProps, "alarmName" | "alarmDescription">) =>
      this.alarms.notify(new cloudwatch.Alarm(this, toPascalCase(name), {
        alarmName: `aura-historia-${config.stage}-host-${name}`,
        alarmDescription: description,
        ...alarmProps,
      }));
    const minute = cdk.Duration.minutes(1);
    const fiveMinutes = cdk.Duration.minutes(5);

    alarm("heartbeat-missing", "The worker published no heartbeat for 5 minutes: the host, Docker or the worker is down, or the host cannot reach AWS.", {
      metric: metric(HOST_METRICS.heartbeat, "SampleCount", minute),
      threshold: 1,
      comparisonOperator: cloudwatch.ComparisonOperator.LESS_THAN_THRESHOLD,
      evaluationPeriods: 5,
      datapointsToAlarm: 5,
      treatMissingData: cloudwatch.TreatMissingData.BREACHING,
    });
    alarm("postgres-unreachable", "PostgreSQL did not answer the worker for 3 minutes: it is down or refuses the runtime role.", {
      metric: metric(HOST_METRICS.postgresReachable, "Maximum", minute),
      threshold: 1,
      comparisonOperator: cloudwatch.ComparisonOperator.LESS_THAN_THRESHOLD,
      evaluationPeriods: 3,
      datapointsToAlarm: 3,
      treatMissingData: cloudwatch.TreatMissingData.NOT_BREACHING,
    });
    for (const [backlogClass, threshold] of Object.entries(monitoring.backlogAgeThresholdSeconds)) {
      alarm(`${backlogClass}-backlog-age`, `A ${backlogClass} job has waited at least ${threshold} s; check the worker and its dependencies with ops inspect.`, {
        metric: metric(HOST_METRICS.oldestJobAge, "Maximum", fiveMinutes, { Class: backlogClass }),
        threshold,
        comparisonOperator: cloudwatch.ComparisonOperator.GREATER_THAN_OR_EQUAL_TO_THRESHOLD,
        evaluationPeriods: 1,
        treatMissingData: cloudwatch.TreatMissingData.NOT_BREACHING,
      });
    }
    // Custody is kept until an operator acts, so a level alarm would stay in alarm and stay
    // silent about every later arrival; this one fires on each increase and clears after it.
    alarm("parked-jobs", "More jobs or outbox snapshots entered failure custody; see docs/durable-worker-runbook.md before any redrive.", {
      metric: new cloudwatch.MathExpression({
        expression: "DIFF(parked)",
        usingMetrics: { parked: metric(HOST_METRICS.parkedJobs, "Maximum", fiveMinutes) },
        period: fiveMinutes,
        label: "Newly parked",
      }),
      threshold: 1,
      comparisonOperator: cloudwatch.ComparisonOperator.GREATER_THAN_OR_EQUAL_TO_THRESHOLD,
      evaluationPeriods: 1,
      treatMissingData: cloudwatch.TreatMissingData.NOT_BREACHING,
    });
    // A rejected command is acknowledged without effect, so neither custody nor backlog sees it.
    const worker = this.logGroups[CONTAINER_LOG_SERVICES.indexOf("worker")];
    const rejected = new logs.MetricFilter(this, "IngestionRejectedFilter", {
      logGroup: worker,
      filterPattern: logs.FilterPattern.literal('{ $.ingestion_outcome = "rejected" }'),
      metricNamespace: config.host.metricNamespace,
      metricName: INGESTION_REJECTED_METRIC,
      metricValue: "1",
    });
    alarm("ingestion-rejected", "ProductListing ingestion rejected a command without applying it; find its command and error code in the worker log.", {
      metric: rejected.metric({ statistic: "Sum", period: fiveMinutes }),
      threshold: 1,
      comparisonOperator: cloudwatch.ComparisonOperator.GREATER_THAN_OR_EQUAL_TO_THRESHOLD,
      evaluationPeriods: 1,
      treatMissingData: cloudwatch.TreatMissingData.NOT_BREACHING,
    });
    alarm("low-free-disk", `The host filesystem has at most ${monitoring.minimumFreeDiskPercent}% free.`, {
      metric: metric(HOST_METRICS.freeDisk, "Minimum", fiveMinutes),
      threshold: monitoring.minimumFreeDiskPercent,
      comparisonOperator: cloudwatch.ComparisonOperator.LESS_THAN_OR_EQUAL_TO_THRESHOLD,
      evaluationPeriods: 1,
      treatMissingData: cloudwatch.TreatMissingData.NOT_BREACHING,
    });
    // install.sh creates each marker when it schedules the job, so a missing figure means the
    // worker cannot see the markers, which is as stale as an old one.
    for (const [job, threshold] of Object.entries(monitoring.maximumJobAgeSeconds)) {
      alarm(`${job}-stale`, `The scheduled ops job ${job} has not succeeded for at least ${threshold} s; see systemctl --failed and journalctl -u aura-prod-ops@${job}.`, {
        metric: metric(HOST_METRICS.sinceLastJobSuccess, "Maximum", fiveMinutes, { Job: job }),
        threshold,
        comparisonOperator: cloudwatch.ComparisonOperator.GREATER_THAN_OR_EQUAL_TO_THRESHOLD,
        evaluationPeriods: 1,
        treatMissingData: cloudwatch.TreatMissingData.BREACHING,
      });
    }
    // No marker means no recorded backup, which is as stale as an old one.
    alarm("backup-stale", `The last successful database backup is at least ${monitoring.maximumBackupAgeSeconds} s old, or none is recorded.`, {
      metric: metric(HOST_METRICS.sinceLastBackup, "Maximum", fiveMinutes),
      threshold: monitoring.maximumBackupAgeSeconds,
      comparisonOperator: cloudwatch.ComparisonOperator.GREATER_THAN_OR_EQUAL_TO_THRESHOLD,
      evaluationPeriods: 1,
      treatMissingData: cloudwatch.TreatMissingData.BREACHING,
    });
  }
}

export interface CrawlerMonitoringProps {
  readonly config: StageConfig;
  readonly alarms: AlarmTopic;
}

/**
 * The host crawler's log group and the alarms docs/crawler/operations.md#metrics-and-alarms
 * requires. Every pass records its metrics, so silence from a crawler that should run breaches.
 */
export class CrawlerMonitoring extends Construct {
  readonly logGroup: logs.LogGroup;

  constructor(scope: Construct, id: string, props: CrawlerMonitoringProps) {
    super(scope, id);
    const { config, alarms } = props;
    const crawler = config.monitoring.crawler;

    // Operational logs only: deleting the stack may delete them.
    this.logGroup = new logs.LogGroup(this, "Logs", {
      logGroupName: crawler.logGroupName,
      retention: crawler.logRetentionDays as logs.RetentionDays,
      removalPolicy: cdk.RemovalPolicy.DESTROY,
    });

    const metric = (metricName: string, statistic: string, period: cdk.Duration) =>
      new cloudwatch.Metric({ namespace: crawler.metricNamespace, metricName, statistic, period });
    const alarm = (name: string, description: string, alarmProps: Omit<cloudwatch.AlarmProps, "alarmName" | "alarmDescription">) =>
      alarms.notify(new cloudwatch.Alarm(this, toPascalCase(name), {
        alarmName: `aura-historia-${config.stage}-crawler-${name}`,
        alarmDescription: description,
        ...alarmProps,
      }));

    alarm("scraper-pass-stalled", `A scraper pass took more than ${crawler.maximumScraperPassMilliseconds} ms, or none finished in 15 minutes: the crawler, its log shipper or its database stalled.`, {
      metric: metric(CRAWLER_METRICS.scraperPassDuration, "Maximum", cdk.Duration.minutes(15)),
      threshold: crawler.maximumScraperPassMilliseconds,
      comparisonOperator: cloudwatch.ComparisonOperator.GREATER_THAN_THRESHOLD,
      evaluationPeriods: 1,
      treatMissingData: cloudwatch.TreatMissingData.BREACHING,
    });
    alarm("no-captures", "No scraped URL produced a capture in 24 hours; see crawler.scraper.pass_completed in the crawler log.", {
      metric: metric(CRAWLER_METRICS.scrapesSucceeded, "Sum", cdk.Duration.days(1)),
      threshold: 0,
      comparisonOperator: cloudwatch.ComparisonOperator.LESS_THAN_OR_EQUAL_TO_THRESHOLD,
      evaluationPeriods: 1,
      treatMissingData: cloudwatch.TreatMissingData.BREACHING,
    });
    alarm("spider-runs-failing", `More than ${crawler.spiderRunFailuresPerHour} spider runs failed in an hour; crawler.spider.crawl_failed in the crawler log names the sources.`, {
      metric: metric(CRAWLER_METRICS.crawlRunsFailed, "Sum", cdk.Duration.hours(1)),
      threshold: crawler.spiderRunFailuresPerHour,
      comparisonOperator: cloudwatch.ComparisonOperator.GREATER_THAN_THRESHOLD,
      evaluationPeriods: 1,
      treatMissingData: cloudwatch.TreatMissingData.NOT_BREACHING,
    });
  }
}

export interface EdgeMonitoringProps {
  readonly config: StageConfig;
}

/** The 5xx share of each public distribution, in the region CloudFront publishes its metrics to. */
export class EdgeMonitoring extends Construct {
  readonly alarms: AlarmTopic;

  constructor(scope: Construct, id: string, props: EdgeMonitoringProps) {
    super(scope, id);
    const { config } = props;
    const monitoring = config.monitoring;
    this.alarms = new AlarmTopic(this, "Alarms", { config });

    for (const [name, distributionId] of Object.entries(monitoring.cloudFrontDistributionIds)) {
      const metric = (metricName: string, statistic: string) => new cloudwatch.Metric({
        namespace: "AWS/CloudFront",
        metricName,
        statistic,
        period: cdk.Duration.minutes(5),
        dimensionsMap: { DistributionId: distributionId, Region: "Global" },
      });
      // A handful of requests makes one failure a large share; such periods do not count.
      const rate = new cloudwatch.MathExpression({
        expression: `IF(requests >= ${monitoring.cloudFrontMinimumRequests}, rate, 0)`,
        usingMetrics: { requests: metric("Requests", "Sum"), rate: metric("5xxErrorRate", "Average") },
        period: cdk.Duration.minutes(5),
        label: `${name}. 5xx rate`,
      });
      this.alarms.notify(new cloudwatch.Alarm(this, `${toPascalCase(name)}5xxRate`, {
        alarmName: `aura-historia-${config.stage}-${name}-cloudfront-5xx-rate`,
        alarmDescription: `At least ${monitoring.cloudFront5xxRatePercent}% of ${name}. requests failed with 5xx for 10 minutes: the host, Caddy or the ${name} process.`,
        metric: rate,
        threshold: monitoring.cloudFront5xxRatePercent,
        comparisonOperator: cloudwatch.ComparisonOperator.GREATER_THAN_OR_EQUAL_TO_THRESHOLD,
        evaluationPeriods: 2,
        datapointsToAlarm: 2,
        treatMissingData: cloudwatch.TreatMissingData.NOT_BREACHING,
      }));
    }
  }
}

function toPascalCase(name: string): string {
  return name.split("-").map((word) => word.charAt(0).toUpperCase() + word.slice(1)).join("");
}
