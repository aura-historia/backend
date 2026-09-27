import * as cdk from "aws-cdk-lib";
import * as ec2 from "aws-cdk-lib/aws-ec2";
import * as ecr from "aws-cdk-lib/aws-ecr";
import * as ecs from "aws-cdk-lib/aws-ecs";
import * as events from "aws-cdk-lib/aws-events";
import * as iam from "aws-cdk-lib/aws-iam";
import * as logs from "aws-cdk-lib/aws-logs";
import * as scheduler from "aws-cdk-lib/aws-scheduler";
import * as secretsmanager from "aws-cdk-lib/aws-secretsmanager";
import * as sqs from "aws-cdk-lib/aws-sqs";
import * as ssm from "aws-cdk-lib/aws-ssm";
import { Construct } from "constructs";
import type { StageConfig } from "../config";
import { WORKLOAD_REGION, ssmValue } from "../config";
import { matcherTaskEventPattern, PERIODIC_MATCHER_CONTAINER, PERIODIC_MATCHER_REPOSITORY, periodicMatcherNames } from "../periodic-matcher-config";
import type { RawEventBridgePattern } from "../periodic-matcher-config";
import type { Network } from "./network";
import type { PostgresConnectionSettings } from "./storage";

type LifecycleClassification = "stopped" | "application-container-nonzero" | "interruption";

export interface PeriodicMatcherProps {
  readonly config: StageConfig;
  readonly network: Network;
  readonly postgres: PostgresConnectionSettings;
  readonly imageDigest: string;
  readonly enabled: cdk.CfnCondition;
  readonly commitSha: string;
}

export class PeriodicMatcher extends Construct {
  readonly deliveryDlq: sqs.Queue;
  readonly taskDefinitionArn: string;

  constructor(scope: Construct, id: string, props: PeriodicMatcherProps) {
    super(scope, id);
    const { config, network, postgres } = props;
    if (config.isEphemeral || !postgres.secretArn || !config.network) throw new Error("Matcher needs real-stage private PostgreSQL credentials and network.");
    const names = periodicMatcherNames(config.stage);
    const stack = cdk.Stack.of(this);
    const repository = ecr.Repository.fromRepositoryName(this, "Repository", PERIODIC_MATCHER_REPOSITORY);
    const runtimeSecret = secretsmanager.Secret.fromSecretCompleteArn(this, "RuntimeSecret", postgres.secretArn);
    // Import by ARN without introducing an AWS::SSM::Parameter::Value<String> CloudFormation
    // parameter: those resolve the secret value during deployment even if ECS uses only its ARN.
    const importParameter = (name: string) => {
      const arn = stack.formatArn({ service: "ssm", resource: "parameter", resourceName: name.slice(1), arnFormat: cdk.ArnFormat.SLASH_RESOURCE_NAME });
      return {
        parameterArn: arn,
        grantRead: (grantee: iam.IGrantable) => iam.Grant.addToPrincipal({ grantee, actions: ["ssm:GetParameters"], resourceArns: [arn] }),
      } as ssm.IParameter;
    };
    const readerUsername = importParameter(`/opensearch/${config.stage}/reader/username`);
    const readerPassword = importParameter(`/opensearch/${config.stage}/reader/password`);
    const googleAdc = importParameter(`/secrets/${config.stage}/google-application-credentials`);
    const retention = config.isProd ? logs.RetentionDays.THREE_MONTHS : logs.RetentionDays.ONE_MONTH;
    const applicationLog = new logs.LogGroup(this, "ApplicationLog", { logGroupName: names.applicationLog, retention, removalPolicy: cdk.RemovalPolicy.RETAIN });
    const lifecycleLog = new logs.LogGroup(this, "LifecycleLog", { logGroupName: names.lifecycleLog, retention, removalPolicy: cdk.RemovalPolicy.RETAIN });
    const cluster = new ecs.Cluster(this, "Cluster", { vpc: network.vpc, clusterName: names.cluster });
    const ecsTaskSourceArn = `arn:${stack.partition}:ecs:${stack.region}:${stack.account}:*`;
    const ecsTaskTrust = () => new iam.ServicePrincipal("ecs-tasks.amazonaws.com", {
      conditions: {
        StringEquals: { "aws:SourceAccount": stack.account },
        ArnLike: { "aws:SourceArn": ecsTaskSourceArn },
      },
    });
    const executionRole = new iam.Role(this, "ExecutionRole", { assumedBy: ecsTaskTrust() });
    const taskRole = new iam.Role(this, "TaskRole", { assumedBy: ecsTaskTrust() });
    repository.grantPull(executionRole);


    applicationLog.grantWrite(executionRole);
    const task = new ecs.FargateTaskDefinition(this, "Task", {
      family: names.family, cpu: 1024, memoryLimitMiB: 2048,
      runtimePlatform: { operatingSystemFamily: ecs.OperatingSystemFamily.LINUX, cpuArchitecture: ecs.CpuArchitecture.X86_64 },
      executionRole, taskRole,
    });
    this.taskDefinitionArn = task.taskDefinitionArn;
    const linuxParameters = new ecs.LinuxParameters(this, "LinuxParameters");
    linuxParameters.dropCapabilities(ecs.Capability.ALL);
    task.addVolume({ name: "tmp" });
    const container = task.addContainer("Matcher", {
      containerName: PERIODIC_MATCHER_CONTAINER,
      image: ecs.ContainerImage.fromRegistry(`${repository.repositoryUri}@${props.imageDigest}`),
      essential: true, user: "10001:10001", readonlyRootFilesystem: true, stopTimeout: cdk.Duration.seconds(120),
      linuxParameters,
      logging: ecs.LogDrivers.awsLogs({ logGroup: applicationLog, streamPrefix: PERIODIC_MATCHER_CONTAINER, mode: ecs.AwsLogDriverMode.NON_BLOCKING, maxBufferSize: cdk.Size.mebibytes(25) }),
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
      secrets: {
        POSTGRES_USERNAME: ecs.Secret.fromSecretsManager(runtimeSecret, "username"),
        POSTGRES_PASSWORD: ecs.Secret.fromSecretsManager(runtimeSecret, "password"),
        OPENSEARCH_USERNAME: ecs.Secret.fromSsmParameter(readerUsername),
        OPENSEARCH_PASSWORD: ecs.Secret.fromSsmParameter(readerPassword),
        AURA_HISTORIA_GOOGLE_ADC_CREDENTIALS_JSON: ecs.Secret.fromSsmParameter(googleAdc),
      },
    });
    container.addMountPoints({ sourceVolume: "tmp", containerPath: "/tmp", readOnly: false });
    this.deliveryDlq = new sqs.Queue(this, "DeliveryDlq", {
      queueName: names.dlq, encryption: sqs.QueueEncryption.SQS_MANAGED, enforceSSL: true,
      retentionPeriod: cdk.Duration.days(14), removalPolicy: config.removalPolicy,
    });
    const group = new scheduler.CfnScheduleGroup(this, "Group", { name: names.group });
    const groupArn = stack.formatArn({ service: "scheduler", resource: "schedule-group", resourceName: names.group });
    const schedulerRole = new iam.Role(this, "SchedulerRole", {
      assumedBy: new iam.ServicePrincipal("scheduler.amazonaws.com", {
        conditions: { StringEquals: { "aws:SourceAccount": stack.account, "aws:SourceArn": groupArn } },
      }),
    });
    schedulerRole.addToPolicy(new iam.PolicyStatement({ actions: ["ecs:RunTask"], resources: [task.taskDefinitionArn], conditions: { ArnEquals: { "ecs:cluster": cluster.clusterArn } } }));
    schedulerRole.addToPolicy(new iam.PolicyStatement({ actions: ["iam:PassRole"], resources: [taskRole.roleArn, executionRole.roleArn], conditions: { StringEquals: { "iam:PassedToService": "ecs-tasks.amazonaws.com" } } }));
    schedulerRole.addToPolicy(new iam.PolicyStatement({ actions: ["sqs:SendMessage"], resources: [this.deliveryDlq.queueArn] }));
    const subnetIds = network.vpc.selectSubnets({ subnetType: ec2.SubnetType.PRIVATE_WITH_EGRESS }).subnetIds;
    const schedule = new scheduler.CfnSchedule(this, "Schedule", {
      name: names.schedule, groupName: group.ref,
      scheduleExpression: "cron(0 15 * * ? *)", scheduleExpressionTimezone: "UTC", flexibleTimeWindow: { mode: "OFF" },
      state: cdk.Fn.conditionIf(props.enabled.logicalId, "ENABLED", "DISABLED").toString(),
      target: {
        arn: cluster.clusterArn, roleArn: schedulerRole.roleArn,
        ecsParameters: {
          taskDefinitionArn: task.taskDefinitionArn, taskCount: 1, launchType: "FARGATE", platformVersion: "1.4.0", enableExecuteCommand: false,
          networkConfiguration: { awsvpcConfiguration: { assignPublicIp: "DISABLED", subnets: subnetIds, securityGroups: [network.applicationSecurityGroup.securityGroupId] } },
        },
        retryPolicy: { maximumEventAgeInSeconds: 3600, maximumRetryAttempts: 2 },
        deadLetterConfig: { arn: this.deliveryDlq.queueArn },
      },
    });
    schedule.node.addDependency(schedulerRole);
    const familyPrefix = stack.formatArn({ service: "ecs", resource: "task-definition", resourceName: `${names.family}:`, arnFormat: cdk.ArnFormat.SLASH_RESOURCE_NAME });
    const lifecycleRuleNames = [names.lifecycleRule, names.exitFailureRule, names.interruptionRule];
    const lifecycleRuleArns = lifecycleRuleNames.map((name) => stack.formatArn({ service: "events", resource: "rule", resourceName: name, arnFormat: cdk.ArnFormat.SLASH_RESOURCE_NAME }));
    const lifecycleLogArn = stack.formatArn({ service: "logs", resource: "log-group", resourceName: names.lifecycleLog, arnFormat: cdk.ArnFormat.COLON_RESOURCE_NAME });
    const lifecyclePolicy = new logs.CfnResourcePolicy(this, "LifecycleEventsLogPolicy", {
      policyName: `periodic-matcher-events-${config.stage}`,
      policyDocument: cdk.Fn.toJsonString(new iam.PolicyDocument({ statements: [new iam.PolicyStatement({
        principals: [new iam.ServicePrincipal("events.amazonaws.com")],
        actions: ["logs:CreateLogStream", "logs:PutLogEvents"],
        resources: [`${lifecycleLogArn}:*`],
        conditions: { ArnEquals: { "aws:SourceArn": lifecycleRuleArns }, StringEquals: { "aws:SourceAccount": stack.account } },
      })] }).toJSON()),
    });
    const lifecycleRule = (logicalId: string, name: string, eventPattern: RawEventBridgePattern, inputTransformer: events.CfnRule.InputTransformerProperty) => {
      const rule = new events.CfnRule(this, logicalId, {
        name, eventPattern,
        targets: [{ id: "LifecycleLog", arn: lifecycleLogArn, inputTransformer }],
      });
      rule.node.addDependency(lifecycleLog, lifecyclePolicy);
      return rule;
    };
    const lifecyclePaths = {
      time: "$.time", taskArn: "$.detail.taskArn", taskDefinitionArn: "$.detail.taskDefinitionArn",
      clusterArn: "$.detail.clusterArn", status: "$.detail.lastStatus",
    };
    const lifecycleInput = (classification: LifecycleClassification, includeStopCode = false): events.CfnRule.InputTransformerProperty => {
      const message = [
        `classification=${classification}`,
        "taskArn=<taskArn>",
        "taskDefinitionArn=<taskDefinitionArn>",
        "clusterArn=<clusterArn>",
        "status=<status>",
        ...(includeStopCode ? ["stopCode=<stopCode>"] : []),
      ].join(" ");
      return {
        inputPathsMap: includeStopCode ? { ...lifecyclePaths, stopCode: "$.detail.stopCode" } : lifecyclePaths,
        inputTemplate: `{"timestamp":<time>,"message":${JSON.stringify(message)}}`,
      };
    };
    const stoppedPattern: RawEventBridgePattern = {
      source: ["aws.ecs"],
      "detail-type": ["ECS Task State Change"],
      detail: { clusterArn: [cluster.clusterArn], taskDefinitionArn: [{ prefix: familyPrefix }], lastStatus: ["STOPPED"] },
    };
    const stoppedRule = lifecycleRule("StoppedTasks", names.lifecycleRule, stoppedPattern, lifecycleInput("stopped"));
    const exitFailureRule = lifecycleRule("FailedTasks", names.exitFailureRule,
      matcherTaskEventPattern(cluster.clusterArn, familyPrefix, "exit"),
      lifecycleInput("application-container-nonzero"));
    const interruptionRule = lifecycleRule("InterruptedTasks", names.interruptionRule,
      matcherTaskEventPattern(cluster.clusterArn, familyPrefix, "interruption"),
      lifecycleInput("interruption", true));
    schedule.node.addDependency(schedulerRole);
    const schedulerPolicy = schedulerRole.node.tryFindChild("DefaultPolicy");
    if (schedulerPolicy) schedule.node.addDependency(schedulerPolicy);

    for (const [key, value] of Object.entries({ ClusterArn: cluster.clusterArn, ClusterName: names.cluster, TaskDefinitionArn: task.taskDefinitionArn, TaskFamily: names.family, ImageDigest: props.imageDigest, ContainerName: PERIODIC_MATCHER_CONTAINER, ApplicationSubnetIds: cdk.Fn.join(",", subnetIds), ApplicationSecurityGroupId: network.applicationSecurityGroup.securityGroupId, ScheduleGroup: names.group, ScheduleName: names.schedule, ScheduleArn: schedule.attrArn, DeliveryDlqUrl: this.deliveryDlq.queueUrl, DeliveryDlqArn: this.deliveryDlq.queueArn, ApplicationLogGroup: applicationLog.logGroupName, LifecycleLogGroup: lifecycleLog.logGroupName })) {
      new cdk.CfnOutput(this, key, { value });
    }
  }
}
