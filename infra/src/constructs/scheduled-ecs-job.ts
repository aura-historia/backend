import * as cdk from "aws-cdk-lib";
import * as ec2 from "aws-cdk-lib/aws-ec2";
import * as ecr from "aws-cdk-lib/aws-ecr";
import * as ecs from "aws-cdk-lib/aws-ecs";
import * as events from "aws-cdk-lib/aws-events";
import * as iam from "aws-cdk-lib/aws-iam";
import * as logs from "aws-cdk-lib/aws-logs";
import * as scheduler from "aws-cdk-lib/aws-scheduler";
import * as sqs from "aws-cdk-lib/aws-sqs";
import { Construct } from "constructs";

import type { Network } from "./network";

export interface ScheduledEcsJobNames {
  readonly cluster: string;
  readonly family: string;
  readonly group: string;
  readonly schedule: string;
  readonly dlq: string;
  readonly applicationLog: string;
  readonly lifecycleLog: string;
  readonly lifecycleRule: string;
  readonly exitFailureRule: string;
  readonly interruptionRule: string;
  readonly lifecyclePolicy: string;
}

export interface ScheduledEcsJobProps {
  readonly network: Network;
  readonly names: ScheduledEcsJobNames;
  readonly imageRepository: string;
  readonly imageDigest: string;
  readonly containerName: string;
  readonly environment: Record<string, string>;
  readonly secrets: (job: Construct) => Record<string, ecs.Secret>;
  readonly scheduleExpression: string;
  readonly enabled: cdk.CfnCondition;
  readonly retention: logs.RetentionDays;
  readonly removalPolicy: cdk.RemovalPolicy;
}

type LifecycleClassification = "stopped" | "application-container-nonzero" | "interruption";
type RawEventBridgePattern = { readonly source: string[]; readonly "detail-type": string[]; readonly detail: Record<string, unknown> };

export class ScheduledEcsJob extends Construct {
  readonly deliveryDlq: sqs.Queue;
  readonly taskDefinitionArn: string;

  constructor(scope: Construct, id: string, props: ScheduledEcsJobProps) {
    super(scope, id);
    const { names, network } = props;
    const stack = cdk.Stack.of(this);
    const repository = ecr.Repository.fromRepositoryName(this, "Repository", props.imageRepository);
    const applicationLog = new logs.LogGroup(this, "ApplicationLog", { logGroupName: names.applicationLog, retention: props.retention, removalPolicy: cdk.RemovalPolicy.RETAIN });
    const lifecycleLog = new logs.LogGroup(this, "LifecycleLog", { logGroupName: names.lifecycleLog, retention: props.retention, removalPolicy: cdk.RemovalPolicy.RETAIN });
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
    const container = task.addContainer("Container", {
      containerName: props.containerName,
      image: ecs.ContainerImage.fromRegistry(`${repository.repositoryUri}@${props.imageDigest}`),
      essential: true, user: "10001:10001", readonlyRootFilesystem: true, stopTimeout: cdk.Duration.seconds(120),
      linuxParameters,
      logging: ecs.LogDrivers.awsLogs({ logGroup: applicationLog, streamPrefix: props.containerName, mode: ecs.AwsLogDriverMode.NON_BLOCKING, maxBufferSize: cdk.Size.mebibytes(25) }),
      environment: props.environment,
      secrets: props.secrets(this),
    });
    container.addMountPoints({ sourceVolume: "tmp", containerPath: "/tmp", readOnly: false });
    this.deliveryDlq = new sqs.Queue(this, "DeliveryDlq", {
      queueName: names.dlq, encryption: sqs.QueueEncryption.SQS_MANAGED, enforceSSL: true,
      retentionPeriod: cdk.Duration.days(14), removalPolicy: props.removalPolicy,
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
      scheduleExpression: props.scheduleExpression, scheduleExpressionTimezone: "UTC", flexibleTimeWindow: { mode: "OFF" },
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
    const schedulerPolicy = schedulerRole.node.tryFindChild("DefaultPolicy");
    if (schedulerPolicy) schedule.node.addDependency(schedulerPolicy);

    const familyPrefix = stack.formatArn({ service: "ecs", resource: "task-definition", resourceName: `${names.family}:`, arnFormat: cdk.ArnFormat.SLASH_RESOURCE_NAME });
    const lifecycleRuleNames = [names.lifecycleRule, names.exitFailureRule, names.interruptionRule];
    const lifecycleRuleArns = lifecycleRuleNames.map((name) => stack.formatArn({ service: "events", resource: "rule", resourceName: name, arnFormat: cdk.ArnFormat.SLASH_RESOURCE_NAME }));
    const lifecycleLogArn = stack.formatArn({ service: "logs", resource: "log-group", resourceName: names.lifecycleLog, arnFormat: cdk.ArnFormat.COLON_RESOURCE_NAME });
    const lifecyclePolicy = new logs.CfnResourcePolicy(this, "LifecycleEventsLogPolicy", {
      policyName: names.lifecyclePolicy,
      policyDocument: cdk.Fn.toJsonString(new iam.PolicyDocument({ statements: [new iam.PolicyStatement({
        principals: [new iam.ServicePrincipal("events.amazonaws.com")],
        actions: ["logs:CreateLogStream", "logs:PutLogEvents"],
        resources: [`${lifecycleLogArn}:*`],
        conditions: { ArnEquals: { "aws:SourceArn": lifecycleRuleArns }, StringEquals: { "aws:SourceAccount": stack.account } },
      })] }).toJSON()),
    });
    const lifecycleRule = (logicalId: string, name: string, eventPattern: RawEventBridgePattern, inputTransformer: events.CfnRule.InputTransformerProperty): void => {
      const rule = new events.CfnRule(this, logicalId, {
        name, eventPattern,
        targets: [{ id: "LifecycleLog", arn: lifecycleLogArn, inputTransformer }],
      });
      rule.node.addDependency(lifecycleLog, lifecyclePolicy);
    };
    const lifecyclePaths = {
      time: "$.time", taskArn: "$.detail.taskArn", taskDefinitionArn: "$.detail.taskDefinitionArn",
      clusterArn: "$.detail.clusterArn", status: "$.detail.lastStatus",
    };
    const lifecycleInput = (classification: LifecycleClassification, includeStopCode = false): events.CfnRule.InputTransformerProperty => {
      const message = [
        `classification=${classification}`,
        "taskArn=<taskArn>", "taskDefinitionArn=<taskDefinitionArn>", "clusterArn=<clusterArn>", "status=<status>",
        ...(includeStopCode ? ["stopCode=<stopCode>"] : []),
      ].join(" ");
      return {
        inputPathsMap: includeStopCode ? { ...lifecyclePaths, stopCode: "$.detail.stopCode" } : lifecyclePaths,
        inputTemplate: `{"timestamp":<time>,"message":${JSON.stringify(message)}}`,
      };
    };
    const baseDetail = { clusterArn: [cluster.clusterArn], taskDefinitionArn: [{ prefix: familyPrefix }], lastStatus: ["STOPPED"] };
    lifecycleRule("StoppedTasks", names.lifecycleRule,
      { source: ["aws.ecs"], "detail-type": ["ECS Task State Change"], detail: baseDetail }, lifecycleInput("stopped"));
    lifecycleRule("FailedTasks", names.exitFailureRule,
      { source: ["aws.ecs"], "detail-type": ["ECS Task State Change"], detail: { ...baseDetail, containers: { name: [props.containerName], exitCode: [{ "anything-but": 0 }] } } },
      lifecycleInput("application-container-nonzero"));
    lifecycleRule("InterruptedTasks", names.interruptionRule,
      { source: ["aws.ecs"], "detail-type": ["ECS Task State Change"], detail: { ...baseDetail, stopCode: ["TaskFailedToStart", "UserInitiated", "ServiceSchedulerInitiated", "SpotInterruption", "TerminationNotice"] } },
      lifecycleInput("interruption", true));

    for (const [key, value] of Object.entries({ ClusterArn: cluster.clusterArn, ClusterName: names.cluster, TaskDefinitionArn: task.taskDefinitionArn, TaskFamily: names.family, ImageDigest: props.imageDigest, ContainerName: props.containerName, ApplicationSubnetIds: cdk.Fn.join(",", subnetIds), ApplicationSecurityGroupId: network.applicationSecurityGroup.securityGroupId, ScheduleGroup: names.group, ScheduleName: names.schedule, ScheduleArn: schedule.attrArn, DeliveryDlqUrl: this.deliveryDlq.queueUrl, DeliveryDlqArn: this.deliveryDlq.queueArn, ApplicationLogGroup: applicationLog.logGroupName, LifecycleLogGroup: lifecycleLog.logGroupName })) {
      new cdk.CfnOutput(this, key, { value });
    }
  }
}
