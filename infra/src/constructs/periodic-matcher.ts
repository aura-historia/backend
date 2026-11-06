import type * as cdk from "aws-cdk-lib";
import type * as ecs from "aws-cdk-lib/aws-ecs";
import * as logs from "aws-cdk-lib/aws-logs";
import type * as sqs from "aws-cdk-lib/aws-sqs";
import { Construct } from "constructs";
import type { StageConfig } from "../config";
import { ssmValue } from "../config";
import { credentialResolver, credentialSecrets } from "../credentials";
import { PERIODIC_MATCHER_CONTAINER, PERIODIC_MATCHER_IMAGE, PERIODIC_MATCHER_REPOSITORY, periodicMatcherNames } from "../periodic-matcher-config";
import type { Network } from "./network";
import { ScheduledEcsJob } from "./scheduled-ecs-job";
import type { PostgresConnectionSettings } from "./storage";

export interface PeriodicMatcherProps {
  readonly config: StageConfig;
  readonly deliveryDlq: sqs.IQueue;
  readonly network: Network;
  readonly cluster: ecs.ICluster;
  readonly postgres: PostgresConnectionSettings;
  readonly imageDigest: string;
  readonly enabled: cdk.CfnCondition;
  readonly commitSha: string;
  readonly classifierModel: string;
  readonly batchCandidates: string;
  readonly shouldShowThresholdBps: string;
}

export class PeriodicMatcher extends ScheduledEcsJob {
  constructor(scope: Construct, id: string, props: PeriodicMatcherProps) {
    const { config, network, postgres } = props;
    if (!config.network) throw new Error("Matcher needs a real-stage private network.");
    const names = periodicMatcherNames(config.stage);
    // Credentials enter the task definition at deployment, like Lambda configuration.
    const credential = credentialResolver(scope);
    const secrets = credentialSecrets(config.stage);
    super(scope, id, {
      network,
      deliveryDlq: props.deliveryDlq,
      cluster: props.cluster,
      names: { ...names, lifecyclePolicy: `periodic-matcher-events-${config.stage}` },
      platform: PERIODIC_MATCHER_IMAGE.platform,
      imageRepository: PERIODIC_MATCHER_REPOSITORY,
      imageDigest: props.imageDigest,
      containerName: PERIODIC_MATCHER_CONTAINER,
      environment: {
        STAGE: config.stage, LOG_LEVEL: "info", POSTGRES_HOST: postgres.host, POSTGRES_PORT: postgres.port,
        POSTGRES_DATABASE: postgres.database, POSTGRES_MAX_CONNECTIONS: "1", POSTGRES_TLS_ROOT_CERT: postgres.tlsRootCert,
        POSTGRES_USERNAME: credential(secrets.postgresRuntime, "username"),
        POSTGRES_PASSWORD: credential(secrets.postgresRuntime, "password"),
        OPENSEARCH_ENDPOINT_URL: config.opensearchEndpointUrl,
        OPENSEARCH_USERNAME: ssmValue(`/opensearch/${config.stage}/reader/username`),
        OPENSEARCH_PASSWORD: credential(secrets.openSearch, "readerPassword"),
        CLOUDFLARE_API_TOKEN: credential(secrets.cloudflare, "workersAiApiToken"),
        CLASSIFIER_MODEL_PROVIDER: config.searchFilterClassifier.provider,
        CLASSIFIER_MODEL: props.classifierModel,
        PERIODIC_MATCH_BATCH_CANDIDATES: props.batchCandidates,
        SEARCH_FILTER_MATCH_SHOULD_SHOW_THRESHOLD_BPS: props.shouldShowThresholdBps,
        CLOUDFLARE_ACCOUNT_ID: ssmValue(`/cloudflare/${config.stage}/account-id`),
        PERIODIC_MATCH_FILTER_PAGE_SIZE: "100", PERIODIC_MATCH_HYBRID_SCAN_LIMIT: "100",
        // Classification budget: never refill to chase accepted matches.
        PERIODIC_MATCH_EVALUATION_LIMIT: "50", PERIODIC_MATCH_CLASSIFICATION_CONCURRENCY: "8",
        PERIODIC_MATCH_MAX_ATTEMPTS: "3", PERIODIC_MATCH_PROJECTION_LAG_SECONDS: "900",
        PERIODIC_MATCH_REPLAY_OVERLAP_SECONDS: "7200", PERIODIC_MATCH_MAX_RUN_SECONDS: "7200",
        AURA_HISTORIA_SOURCE_REVISION: props.commitSha,
      },
      scheduleExpression: "cron(0 15 * * ? *)",
      enabled: props.enabled,
      retention: config.isProd ? logs.RetentionDays.THREE_MONTHS : logs.RetentionDays.ONE_MONTH,
      removalPolicy: config.removalPolicy,
    });
  }
}
