import * as cdk from "aws-cdk-lib";
import * as ecs from "aws-cdk-lib/aws-ecs";
import * as iam from "aws-cdk-lib/aws-iam";
import * as logs from "aws-cdk-lib/aws-logs";
import * as secretsmanager from "aws-cdk-lib/aws-secretsmanager";
import * as ssm from "aws-cdk-lib/aws-ssm";
import { Construct } from "constructs";
import type { StageConfig } from "../config";
import { ssmValue } from "../config";
import { PERIODIC_MATCHER_CONTAINER, PERIODIC_MATCHER_REPOSITORY, periodicMatcherNames } from "../periodic-matcher-config";
import type { Network } from "./network";
import { ScheduledEcsJob } from "./scheduled-ecs-job";
import type { PostgresConnectionSettings } from "./storage";

export interface PeriodicMatcherProps {
  readonly config: StageConfig;
  readonly network: Network;
  readonly postgres: PostgresConnectionSettings;
  readonly imageDigest: string;
  readonly enabled: cdk.CfnCondition;
  readonly commitSha: string;
}

export class PeriodicMatcher extends ScheduledEcsJob {
  constructor(scope: Construct, id: string, props: PeriodicMatcherProps) {
    const { config, network, postgres } = props;
    const secretArn = postgres.secretArn;
    if (config.isEphemeral || !secretArn || !config.network) throw new Error("Matcher needs real-stage private PostgreSQL credentials and network.");
    const names = periodicMatcherNames(config.stage);
    super(scope, id, {
      network,
      names: { ...names, lifecyclePolicy: `periodic-matcher-events-${config.stage}` },
      imageRepository: PERIODIC_MATCHER_REPOSITORY,
      imageDigest: props.imageDigest,
      containerName: PERIODIC_MATCHER_CONTAINER,
      environment: {
        STAGE: config.stage, LOG_LEVEL: "info", POSTGRES_HOST: postgres.host, POSTGRES_PORT: postgres.port,
        POSTGRES_DATABASE: postgres.database, POSTGRES_MAX_CONNECTIONS: "1", POSTGRES_TLS_ROOT_CERT: postgres.tlsRootCert,
        OPENSEARCH_ENDPOINT_URL: config.opensearchEndpointUrl,
        VERTEX_AI_PROJECT_ID: ssmValue(`/vertex-ai/${config.stage}/project-id`),
        VERTEX_AI_LOCATION: ssmValue(`/vertex-ai/${config.stage}/location`),
        VERTEX_AI_MODEL: ssmValue(`/vertex-ai/${config.stage}/model`),
        PERIODIC_MATCH_FILTER_PAGE_SIZE: "100", PERIODIC_MATCH_HYBRID_SCAN_LIMIT: "100",
        PERIODIC_MATCH_EVALUATION_LIMIT: "50", PERIODIC_MATCH_LLM_CONCURRENCY: "8",
        PERIODIC_MATCH_MAX_ATTEMPTS: "3", PERIODIC_MATCH_PROJECTION_LAG_SECONDS: "900",
        PERIODIC_MATCH_REPLAY_OVERLAP_SECONDS: "7200", PERIODIC_MATCH_MAX_RUN_SECONDS: "7200",
        AURA_HISTORIA_SOURCE_REVISION: props.commitSha,
      },
      secrets: (job) => {
        const stack = cdk.Stack.of(job);
        const runtimeSecret = secretsmanager.Secret.fromSecretCompleteArn(job, "RuntimeSecret", secretArn);
        // Import by ARN without resolving the secret value as a CloudFormation parameter.
        const importParameter = (name: string) => {
          const arn = stack.formatArn({ service: "ssm", resource: "parameter", resourceName: name.slice(1), arnFormat: cdk.ArnFormat.SLASH_RESOURCE_NAME });
          return {
            parameterArn: arn,
            grantRead: (grantee: iam.IGrantable) => iam.Grant.addToPrincipal({ grantee, actions: ["ssm:GetParameters"], resourceArns: [arn] }),
          } as ssm.IParameter;
        };
        return {
          POSTGRES_USERNAME: ecs.Secret.fromSecretsManager(runtimeSecret, "username"),
          POSTGRES_PASSWORD: ecs.Secret.fromSecretsManager(runtimeSecret, "password"),
          OPENSEARCH_USERNAME: ecs.Secret.fromSsmParameter(importParameter(`/opensearch/${config.stage}/reader/username`)),
          OPENSEARCH_PASSWORD: ecs.Secret.fromSsmParameter(importParameter(`/opensearch/${config.stage}/reader/password`)),
          AURA_HISTORIA_GOOGLE_ADC_CREDENTIALS_JSON: ecs.Secret.fromSsmParameter(importParameter(`/secrets/${config.stage}/google-application-credentials`)),
        };
      },
      scheduleExpression: "cron(0 15 * * ? *)",
      enabled: props.enabled,
      retention: config.isProd ? logs.RetentionDays.THREE_MONTHS : logs.RetentionDays.ONE_MONTH,
      removalPolicy: config.removalPolicy,
    });
  }
}
