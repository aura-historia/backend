import * as cdk from "aws-cdk-lib";
import * as dynamodb from "aws-cdk-lib/aws-dynamodb";
import * as ec2 from "aws-cdk-lib/aws-ec2";
import * as ecr from "aws-cdk-lib/aws-ecr";
import * as ecs from "aws-cdk-lib/aws-ecs";
import * as events from "aws-cdk-lib/aws-events";
import * as targets from "aws-cdk-lib/aws-events-targets";
import * as iam from "aws-cdk-lib/aws-iam";
import * as logs from "aws-cdk-lib/aws-logs";
import * as s3 from "aws-cdk-lib/aws-s3";
import { Construct } from "constructs";
import type { StageConfig } from "../config";
import { ssmValue } from "../config";
import type { Search } from "./opensearch";
import type { PostgresConnectionSettings } from "./storage";

const IMAGE_REPOSITORY_NAME = "aura-historia-search-filter-periodic-match";
const IMAGE_NAME = "search-filter-periodic-match";
const GEMINI_MODEL = "gemini-3.1-flash-lite";

export interface PeriodicSearchFilterMatchingProps {
  readonly config: StageConfig;
  readonly commitSha: string;
  readonly table: dynamodb.Table;
  readonly postgres: PostgresConnectionSettings;
  readonly mailTemplateBucket: s3.IBucket;
  readonly search: Search;
}

export class PeriodicSearchFilterMatching extends Construct {
  constructor(scope: Construct, id: string, props: PeriodicSearchFilterMatchingProps) {
    super(scope, id);

    if (props.config.isEphemeral) {
      return;
    }

    const stageName = props.config.stage;
    const vpc = new ec2.Vpc(this, "Vpc", {
      maxAzs: 2,
      natGateways: 0,
      subnetConfiguration: [
        {
          name: "public",
          subnetType: ec2.SubnetType.PUBLIC,
        },
      ],
    });
    const cluster = new ecs.Cluster(this, "Cluster", {
      clusterName: `${IMAGE_NAME}-${stageName}`,
      vpc,
    });
    const repository = ecr.Repository.fromRepositoryName(this, "Repository", IMAGE_REPOSITORY_NAME);

    const taskDefinition = new ecs.FargateTaskDefinition(this, "TaskDefinition", {
      family: `${IMAGE_NAME}-${stageName}`,
      cpu: 1024,
      memoryLimitMiB: 2048,
    });
    props.table.grantReadWriteData(taskDefinition.taskRole);
    props.search.grantRead(taskDefinition.taskRole);
    props.mailTemplateBucket.grantRead(taskDefinition.taskRole);
    taskDefinition.taskRole.addToPrincipalPolicy(
      new iam.PolicyStatement({
        actions: ["ses:SendEmail", "ses:SendRawEmail"],
        resources: ["*"],
      }),
    );
    repository.grantPull(taskDefinition.obtainExecutionRole());

    const logGroup = new logs.LogGroup(this, "LogGroup", {
      logGroupName: `/ecs/${IMAGE_NAME}-${stageName}`,
      retention: logs.RetentionDays.ONE_MONTH,
      removalPolicy: props.config.removalPolicy,
    });

    taskDefinition.addContainer("Container", {
      containerName: IMAGE_NAME,
      image: ecs.ContainerImage.fromEcrRepository(repository, `${stageName}-${props.commitSha}`),
      logging: ecs.LogDrivers.awsLogs({
        logGroup,
        streamPrefix: IMAGE_NAME,
      }),
      environment: withPostgresEnvironment(props.postgres, withOpenSearchCredentials(props.config, {
        COMMIT_SHA: props.commitSha,
        DYNAMODB_TABLE_NAME: props.table.tableName,
        GEMINI_API_KEY: ssmSecret(props.config, "gemini-api-key"),
        GEMINI_MODEL,
        OPENSEARCH_ENDPOINT_URL: props.search.endpointUrl,
        PERIODIC_MATCH_LLM_CONCURRENCY: "50",
        S3_BUCKET_NAME_TEMPLATES: props.mailTemplateBucket.bucketName,
        STAGE_NAME: stageName,
      })),
    });

    new events.Rule(this, "Schedule", {
      schedule: events.Schedule.expression("cron(0 15 * * ? *)"),
      targets: [
        new targets.EcsTask({
          cluster,
          taskDefinition,
          assignPublicIp: true,
          subnetSelection: {
            subnetType: ec2.SubnetType.PUBLIC,
          },
          taskCount: 1,
          maxEventAge: cdk.Duration.hours(1),
          retryAttempts: 2,
        }),
      ],
    });
  }
}

function withPostgresEnvironment(postgres: PostgresConnectionSettings, env: Record<string, string>): Record<string, string> {
  return {
    ...env,
    POSTGRES_DATABASE: postgres.database,
    POSTGRES_HOST: postgres.host,
    POSTGRES_MAX_CONNECTIONS: postgres.maxConnections,
    POSTGRES_PASSWORD: postgres.password,
    POSTGRES_PORT: postgres.port,
    POSTGRES_USERNAME: postgres.username,
  };
}

function withOpenSearchCredentials(config: StageConfig, env: Record<string, string>): Record<string, string> {
  return {
    ...env,
    OPENSEARCH_USERNAME: ssmValue(`/opensearch/${config.stage}/username`),
    OPENSEARCH_PASSWORD: ssmValue(`/opensearch/${config.stage}/password`),
  };
}

function ssmSecret(config: StageConfig, name: string): string {
  return ssmValue(`/secrets/${config.stage}/${name}`);
}
