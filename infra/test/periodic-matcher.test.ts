import * as cdk from "aws-cdk-lib";
import * as iam from "aws-cdk-lib/aws-iam";
import { Match, Template } from "aws-cdk-lib/assertions";
import { createApplicationStacks } from "../src/application-stack";
import { stageConfig } from "../src/config";
import { ScheduledEcsJob, ecsCpuArchitectureForPlatform } from "../src/constructs/scheduled-ecs-job";
import { ecsTaskEventPattern, ecsTaskL2EventPattern } from "../src/constructs/ecs-task-event-patterns";
import { matcherTaskEventPattern, periodicMatcherNames, PERIODIC_MATCHER_IMAGE } from "../src/periodic-matcher-config";
import { scheduledEcsClusterName } from "../src/scheduled-ecs-config";

function stacks(stage: "dev" | "prod" | "ephemeral") {
  return createApplicationStacks(new cdk.App({ analyticsReporting: false }), { stage });
}

type FixtureStage = "dev" | "prod";
type Transformer = { InputPathsMap: Record<string, string>; InputTemplate: string };

function fixtureTaskEvent(stage: FixtureStage, detail: Record<string, unknown> = {}) {
  return {
    source: "aws.ecs",
    "detail-type": "ECS Task State Change",
    time: "2026-09-27T15:00:00Z",
    detail: {
      clusterArn: `arn:aws:ecs:eu-central-1:123456789012:cluster/aura-historia-scheduled-${stage}`,
      taskArn: `arn:aws:ecs:eu-central-1:123456789012:task/aura-historia-scheduled-${stage}/0123456789abcdef0`,
      taskDefinitionArn: `arn:aws:ecs:eu-central-1:123456789012:task-definition/aura-historia-periodic-matcher-${stage}:42`,
      lastStatus: "STOPPED",
      reason: "PRIVATE_REASON_SHOULD_NOT_BE_LOGGED",
      error: "PRIVATE_ERROR_SHOULD_NOT_BE_LOGGED",
      ...detail,
    },
  };
}

function valueAtPath(event: unknown, path: string): unknown {
  const segments = path.replace(/^\$\.?/, "").split(".");
  let value = event;
  for (const segment of segments) {
    if (!value || typeof value !== "object" || !Object.prototype.hasOwnProperty.call(value, segment)) return undefined;
    value = (value as Record<string, unknown>)[segment];
  }
  return value;
}

function isInsideJsonString(input: string, position: number): boolean {
  let inside = false;
  let escaped = false;
  for (let index = 0; index < position; index += 1) {
    const character = input[index];
    if (escaped) {
      escaped = false;
    } else if (character === "\\") {
      escaped = true;
    } else if (character === '"') {
      inside = !inside;
    }
  }
  return inside;
}

function renderInputTransformer(transformer: Transformer, event: unknown): Record<string, unknown> {
  const template = transformer.InputTemplate;
  let rendered = "";
  let cursor = 0;
  while (cursor < template.length) {
    const placeholderStart = template.indexOf("<", cursor);
    if (placeholderStart === -1) {
      rendered += template.slice(cursor);
      break;
    }
    const placeholderEnd = template.indexOf(">", placeholderStart + 1);
    if (placeholderEnd === -1) throw new Error("Unclosed EventBridge input transformer placeholder.");
    rendered += template.slice(cursor, placeholderStart);
    const pathName = template.slice(placeholderStart + 1, placeholderEnd);
    const path = transformer.InputPathsMap[pathName];
    if (!path) throw new Error(`Input transformer placeholder '${pathName}' has no input path.`);
    const value = valueAtPath(event, path);
    if (value === undefined) throw new Error(`Input transformer path '${path}' is missing from its fixture event.`);
    const encoded = JSON.stringify(value);
    if (encoded === undefined) throw new Error(`Input transformer path '${path}' cannot be serialized.`);
    if (isInsideJsonString(template, placeholderStart)) {
      if (typeof value !== "string") throw new Error(`Input transformer path '${path}' must be a string inside the message.`);
      rendered += encoded.slice(1, -1);
    } else {
      rendered += encoded;
    }
    cursor = placeholderEnd + 1;
  }
  return JSON.parse(rendered) as Record<string, unknown>;
}

function synthesizedRule(template: Template, name: string): Record<string, any> {
  const matches = Object.values(template.findResources("AWS::Events::Rule"))
    .filter((rule) => rule.Properties.Name === name);
  expect(matches).toHaveLength(1);
  return matches[0];
}

function expectRawTaskPattern(pattern: Record<string, any>, stage: FixtureStage, detailKeys: string[]): void {
  expect(Object.keys(pattern).sort()).toEqual(["detail", "detail-type", "source"]);
  expect(pattern.source).toEqual(["aws.ecs"]);
  expect(pattern["detail-type"]).toEqual(["ECS Task State Change"]);
  expect(pattern).not.toHaveProperty("detailType");
  expect(Object.keys(pattern.detail).sort()).toEqual(detailKeys.sort());
  expect(pattern.detail.clusterArn).toHaveLength(1);
  expect(JSON.stringify(pattern.detail.taskDefinitionArn)).toContain(`aura-historia-periodic-matcher-${stage}:`);
  expect(pattern.detail.lastStatus).toEqual(["STOPPED"]);
}

describe.each(["dev", "prod"] as const)("%s periodic matcher", (stage) => {
  test("requires immutable digest, starts disabled, runs one private Fargate task daily", () => {
    const stack = stacks(stage);
    const template = Template.fromStack(stack.compute);
    expect(template.toJSON().Parameters.PeriodicMatcherImageDigest).toMatchObject({ Type: "String", AllowedPattern: "^sha256:[0-9a-f]{64}$" });
    expect(template.toJSON().Parameters.PeriodicMatcherImageDigest.Default).toBeUndefined();
    expect(template.toJSON().Parameters.PeriodicMatcherEnabled).toMatchObject({ Default: "false", AllowedValues: ["true", "false"] });
    const classifierConfig = stageConfig(stage).searchFilterClassifier;
    expect(template.toJSON().Parameters.SearchFilterClassifierModel).toMatchObject({
      Default: classifierConfig.model,
      AllowedValues: ["clef-flash", "clef"],
    });
    expect(template.toJSON().Parameters.SearchFilterMatchShouldShowThresholdBps).toMatchObject({
      Default: classifierConfig.shouldShowThresholdBps,
      MinValue: 0,
      MaxValue: 10_000,
    });
    template.hasResourceProperties("AWS::ECS::TaskDefinition", {
      Cpu: "1024", Memory: "2048", NetworkMode: "awsvpc", RequiresCompatibilities: ["FARGATE"],
      RuntimePlatform: { CpuArchitecture: "X86_64", OperatingSystemFamily: "LINUX" },
      ContainerDefinitions: Match.arrayWith([Match.objectLike({
        Name: "periodic-matcher", Essential: true, ReadonlyRootFilesystem: true, User: "10001:10001", StopTimeout: 120,
        Command: Match.absent(), PortMappings: Match.absent(),
        MountPoints: [{ SourceVolume: "tmp", ContainerPath: "/tmp", ReadOnly: false }],
        LinuxParameters: { Capabilities: { Drop: ["ALL"] } },
        LogConfiguration: Match.objectLike({ LogDriver: "awslogs", Options: Match.objectLike({ "mode": "non-blocking" }) }),
      })]),
    });
    const taskDefinitions = Object.entries(template.findResources("AWS::ECS::TaskDefinition"));
    const [taskDefinitionId, taskDefinition] = taskDefinitions[0];
    const task = taskDefinition.Properties;
    const matcherEnvironment = Object.fromEntries(
      task.ContainerDefinitions[0].Environment.map((entry: { Name: string; Value: string }) => [entry.Name, entry.Value]),
    );
    expect(matcherEnvironment).toMatchObject({
      CLASSIFIER_MODEL_PROVIDER: classifierConfig.provider,
      CLASSIFIER_MODEL: { Ref: "SearchFilterClassifierModel" },
      SEARCH_FILTER_MATCH_SHOULD_SHOW_THRESHOLD_BPS: {
        Ref: "SearchFilterMatchShouldShowThresholdBps",
      },
    });
    expect(taskDefinitions).toHaveLength(1);
    expect(PERIODIC_MATCHER_IMAGE.digestParameter).toBe("PeriodicMatcherImageDigest");
    expect(PERIODIC_MATCHER_IMAGE.taskDefinitionOutput).toBe("PeriodicMatcherTaskDefinitionArn");
    expect(template.toJSON().Outputs[PERIODIC_MATCHER_IMAGE.taskDefinitionOutput].Value).toEqual({ Ref: taskDefinitionId });
    expect(task.ContainerDefinitions).toHaveLength(1);
    expect(JSON.stringify(task.ContainerDefinitions[0].Image)).toContain("PeriodicMatcherImageDigest");
    expect(JSON.stringify(task.ContainerDefinitions[0].Image)).toContain("aura-historia-periodic-matcher");
    expect(task.ContainerDefinitions[0].Secrets).toHaveLength(5);
    const environmentNames = task.ContainerDefinitions[0].Environment.map((entry: { Name: string }) => entry.Name);
    for (const forbiddenName of ["POSTGRES_PASSWORD", "OPENSEARCH_PASSWORD", "CLOUDFLARE_API_TOKEN"]) {
      expect(environmentNames).not.toContain(forbiddenName);
    }
    expect(JSON.stringify(task.ContainerDefinitions[0].Secrets)).toContain(`/opensearch/${stage}/reader/password`);
    expect(JSON.stringify(task.ContainerDefinitions[0].Secrets)).toContain(`/secrets/${stage}/cloudflare-workers-ai-api-token`);
    expect(Object.keys(template.toJSON().Parameters).filter((name: string) => name.startsWith("PeriodicMatcher"))).toEqual(["PeriodicMatcherImageDigest", "PeriodicMatcherEnabled"]);
    const roles = Object.entries(template.findResources("AWS::IAM::Role"));
    const taskRole = roles.find(([id]) => id.includes("PeriodicMatcherTaskRole"));
    const executionRole = roles.find(([id]) => id.includes("PeriodicMatcherExecutionRole"));
    const schedulerRole = roles.find(([id]) => id.includes("PeriodicMatcherSchedulerRole"));
    expect(taskRole).toBeDefined();
    expect(executionRole).toBeDefined();
    expect(schedulerRole).toBeDefined();
    for (const role of [taskRole![1], executionRole![1]]) {
      const trust = JSON.stringify(role.Properties.AssumeRolePolicyDocument);
      expect(trust).toContain("ecs-tasks.amazonaws.com");
      expect(trust).toContain("aws:SourceAccount");
      expect(trust).toContain("aws:SourceArn");
    }
    const schedulerTrust = JSON.stringify(schedulerRole![1].Properties.AssumeRolePolicyDocument);
    expect(schedulerTrust).toContain("scheduler.amazonaws.com");
    expect(schedulerTrust).toContain("aws:SourceAccount");
    expect(schedulerTrust).toContain("aws:SourceArn");
    expect(schedulerTrust).toContain(`aura-historia-periodic-matcher-${stage}`);
    const policies = Object.entries(template.findResources("AWS::IAM::Policy"));
    expect(policies.some(([id]) => id.includes("PeriodicMatcherTaskRole"))).toBe(false);
    const [schedulerPolicyId, schedulerPolicy] = policies.find(([id]) => id.includes("PeriodicMatcherSchedulerRole"))!;
    const schedulerStatements = schedulerPolicy.Properties.PolicyDocument.Statement;
    expect(schedulerStatements.map((statement: { Action: string }) => statement.Action)).toEqual(["ecs:RunTask", "iam:PassRole", "sqs:SendMessage"]);
    expect(JSON.stringify(schedulerStatements)).not.toContain('"Resource":"*"');
    expect(JSON.stringify(schedulerStatements[0])).toContain("ecs:cluster");
    expect(JSON.stringify(schedulerStatements[1])).toContain("iam:PassedToService");
    const names = periodicMatcherNames(stage);
    const schedule = Object.values(template.findResources("AWS::Scheduler::Schedule")).find((resource) => resource.Properties.Name === names.schedule)!;
    const [groupId] = Object.entries(template.findResources("AWS::Scheduler::ScheduleGroup"))
      .find(([, resource]) => resource.Properties.Name === names.group)!;
    expect(schedule.Properties.GroupName).toEqual({ Ref: groupId });
    expect(schedule.DependsOn).toContain(schedulerPolicyId);
    const scheduleArnOutputs = Object.entries(template.toJSON().Outputs as Record<string, { Value: unknown }>)
      .filter(([id]) => id.startsWith("PeriodicMatcherScheduleArn"));
    expect(scheduleArnOutputs).toHaveLength(1);
    expect(scheduleArnOutputs[0][1].Value).toEqual({
      "Fn::Join": ["", [
        "arn:", { Ref: "AWS::Partition" }, ":scheduler:", { Ref: "AWS::Region" },
        ":", { Ref: "AWS::AccountId" }, `:schedule/${names.group}/${names.schedule}`,
      ]],
    });
    expect(JSON.stringify(template.toJSON())).not.toContain("PeriodicMatcherReaderPasswordParameter");
    template.resourceCountIs("AWS::Logs::ResourcePolicy", 1);
    expect(Object.keys(template.findResources("AWS::CloudFormation::CustomResource")).filter((key) => key.includes("PeriodicMatcher"))).toHaveLength(0);
    template.hasResourceProperties("AWS::Scheduler::Schedule", {
      ScheduleExpression: "cron(0 15 * * ? *)", ScheduleExpressionTimezone: "UTC", FlexibleTimeWindow: { Mode: "OFF" },
      Target: Match.objectLike({ EcsParameters: Match.objectLike({ TaskCount: 1, LaunchType: "FARGATE", PlatformVersion: "1.4.0", NetworkConfiguration: Match.objectLike({ AwsvpcConfiguration: Match.objectLike({ AssignPublicIp: "DISABLED" }) }) }), RetryPolicy: { MaximumEventAgeInSeconds: 3600, MaximumRetryAttempts: 2 } }),
    });
    template.resourceCountIs("AWS::ECS::Service", 0);
    const stoppedRule = synthesizedRule(template, names.lifecycleRule);
    const exitRule = synthesizedRule(template, names.exitFailureRule);
    const interruptionRule = synthesizedRule(template, names.interruptionRule);
    const lifecycleRules = [stoppedRule, exitRule, interruptionRule];
    expect(lifecycleRules).toHaveLength(3);
    expectRawTaskPattern(stoppedRule.Properties.EventPattern, stage, ["clusterArn", "taskDefinitionArn", "lastStatus"]);
    const [clusterId, clusterResource] = Object.entries(template.findResources("AWS::ECS::Cluster"))[0];
    expect(clusterId).toBe("PeriodicMatcherCluster207C1F86");
    expect(clusterResource.Properties.ClusterName).toBe(scheduledEcsClusterName(stage));
    expect(stoppedRule.Properties.EventPattern.detail.clusterArn).toEqual([{ "Fn::GetAtt": [clusterId, "Arn"] }]);
    expect(stoppedRule.Properties.EventPattern.detail).toMatchObject({
      taskDefinitionArn: [{ prefix: expect.anything() }],
    });
    expectRawTaskPattern(exitRule.Properties.EventPattern, stage, ["clusterArn", "containers", "lastStatus", "taskDefinitionArn"]);
    expect(exitRule.Properties.EventPattern.detail.containers).toEqual({
      name: ["periodic-matcher"],
      exitCode: [{ "anything-but": 0 }],
    });
    expectRawTaskPattern(interruptionRule.Properties.EventPattern, stage, ["clusterArn", "lastStatus", "stopCode", "taskDefinitionArn"]);
    expect(interruptionRule.Properties.EventPattern.detail.stopCode).toEqual([
      "TaskFailedToStart", "UserInitiated", "ServiceSchedulerInitiated", "SpotInterruption", "TerminationNotice",
    ]);
    expect(JSON.stringify(lifecycleRules.map((rule) => rule.Properties.Targets))).not.toMatch(/lambda\.amazonaws\.com/i);

    const lifecyclePolicy = Object.values(template.findResources("AWS::Logs::ResourcePolicy"))[0];
    const policyDocument = JSON.stringify(lifecyclePolicy.Properties.PolicyDocument);
    expect(lifecyclePolicy.Properties.PolicyName).toBe(`periodic-matcher-events-${stage}`);
    expect(policyDocument).toContain(`/aura-historia/${stage}/periodic-matcher-lifecycle:*`);
    expect(policyDocument).toContain("logs:CreateLogStream");
    expect(policyDocument).toContain("logs:PutLogEvents");
    expect(policyDocument).toContain("aws:SourceArn");
    expect(policyDocument).toContain("aws:SourceAccount");
    for (const ruleName of [names.lifecycleRule, names.exitFailureRule, names.interruptionRule]) {
      expect(policyDocument).toContain(ruleName);
    }
    expect(policyDocument).not.toContain('"Resource":"*"');
    expect(Object.keys(template.findResources("AWS::Lambda::Function")).filter((logicalId) => logicalId.includes("PeriodicMatcher"))).toHaveLength(0);

    const successfulEvent = fixtureTaskEvent(stage, { containers: [{ name: "periodic-matcher", exitCode: 0 }], stopCode: "EssentialContainerExited" });
    const failedEvent = fixtureTaskEvent(stage, { containers: [{ name: "periodic-matcher", exitCode: 17 }], stopCode: "EssentialContainerExited" });
    const startupEvent = fixtureTaskEvent(stage, { stopCode: "TaskFailedToStart" });
    const basePaths = {
      time: "$.time",
      taskArn: "$.detail.taskArn",
      taskDefinitionArn: "$.detail.taskDefinitionArn",
      clusterArn: "$.detail.clusterArn",
      status: "$.detail.lastStatus",
    };
    const logFixtures = [
      { rule: stoppedRule, event: successfulEvent, classification: "stopped", paths: basePaths, stopCode: false },
      { rule: exitRule, event: failedEvent, classification: "application-container-nonzero", paths: basePaths, stopCode: false },
      { rule: interruptionRule, event: startupEvent, classification: "interruption", paths: { ...basePaths, stopCode: "$.detail.stopCode" }, stopCode: true },
    ];
    for (const { rule, event, classification, paths, stopCode } of logFixtures) {
      const transformer = rule.Properties.Targets[0].InputTransformer as Transformer;
      expect(transformer.InputPathsMap).toEqual(paths);
      expect(transformer.InputTemplate).not.toMatch(/containers|exitCode|reason|error|credential|password|secret/i);
      const envelope = renderInputTransformer(transformer, event);
      expect(Object.keys(envelope).sort()).toEqual(["message", "timestamp"]);
      expect(envelope.timestamp).toBe(event.time);
      const detail = event.detail as Record<string, unknown>;
      const expectedMessage = `classification=${classification} taskArn=${detail.taskArn} taskDefinitionArn=${detail.taskDefinitionArn} clusterArn=${detail.clusterArn} status=${detail.lastStatus}${stopCode ? ` stopCode=${detail.stopCode}` : ""}`;
      expect(envelope.message).toBe(expectedMessage);
      expect(envelope.message).not.toContain("PRIVATE_REASON_SHOULD_NOT_BE_LOGGED");
      expect(envelope.message).not.toContain("PRIVATE_ERROR_SHOULD_NOT_BE_LOGGED");
      if (stopCode) {
        expect(envelope.message).toContain("stopCode=TaskFailedToStart");
      } else {
        expect(envelope.message).not.toContain("stopCode=");
      }
    }
    expect(logFixtures[0].rule.Properties.Targets[0].InputTransformer.InputTemplate).not.toContain("stopCode");
    expect(logFixtures[1].rule.Properties.Targets[0].InputTransformer.InputTemplate).not.toContain("stopCode");
    template.hasResourceProperties("AWS::SQS::Queue", { QueueName: `aura-historia-periodic-matcher-delivery-${stage}`, SqsManagedSseEnabled: true, MessageRetentionPeriod: 1209600 });
    template.resourceCountIs("AWS::ECR::Repository", 0);
    expect(stack.compute.periodicMatcher).toBeDefined();
    expect(JSON.stringify(Template.fromStack(stack.network!).toJSON())).toContain("starport-layer-bucket/*");
  });
});

test("synthesized lifecycle log targets emit a timestamp and string message, never a detail wrapper", () => {
  const template = Template.fromStack(stacks("dev").compute).toJSON();
  const rules = Object.values(template.Resources as Record<string, { Type: string; Properties: Record<string, any> }>)
    .filter((resource) => resource.Type === "AWS::Events::Rule" && resource.Properties.Name?.includes("periodic-matcher"));
  expect(rules).toHaveLength(3);
  for (const rule of rules) {
    const transformer = rule.Properties.Targets[0].InputTransformer as Transformer;
    expect(transformer.InputTemplate).toMatch(/^\{"timestamp":<time>,"message":"classification=/);
    expect(transformer.InputTemplate).not.toMatch(/"detail"|"message":\{|"message":<detail>/);
    const event = fixtureTaskEvent("dev", { stopCode: "TaskFailedToStart" });
    const payload = renderInputTransformer(transformer, event);
    expect(Object.keys(payload).sort()).toEqual(["message", "timestamp"]);
    expect(payload.timestamp).toBe(event.time);
    expect(typeof payload.message).toBe("string");
  }
});

test("two scheduled ECS jobs share one stage cluster but retain independent tasks, policies and lifecycle targets", () => {
  const app = stacks("dev");
  const names = periodicMatcherNames("dev");
  const secondNames = Object.fromEntries(Object.entries(names).map(([key, value]) => [key, value.replaceAll("periodic-matcher", "example-job").replaceAll("search-filter-periodic-match", "example-job")])) as unknown as typeof names;
  const armImage = { ...PERIODIC_MATCHER_IMAGE, platform: "linux/arm64" as const };
  new ScheduledEcsJob(app.compute, "ExampleJob", {
    network: app.network!.network, cluster: app.compute.scheduledEcsCluster!,
    names: { ...secondNames, lifecyclePolicy: "example-job-events-dev" },
    platform: armImage.platform, cpu: 512, memoryLimitMiB: 1024,
    imageRepository: "example-job", imageDigest: `sha256:${"a".repeat(64)}`,
    containerName: "example-job", environment: { STAGE: "dev" }, secrets: () => ({}),
    extendTaskRole: (role) => role.addToPolicy(new iam.PolicyStatement({ actions: ["s3:GetObject"], resources: ["arn:aws:s3:::example-bucket/input/*"] })),
    scheduleExpression: "cron(0 12 * * ? *)",
    enabled: new cdk.CfnCondition(app.compute, "ExampleJobActivation", { expression: cdk.Fn.conditionEquals("true", "false") }),
    retention: cdk.aws_logs.RetentionDays.ONE_MONTH, removalPolicy: cdk.RemovalPolicy.RETAIN,
  });
  const template = Template.fromStack(app.compute);
  template.resourceCountIs("AWS::ECS::Cluster", 1);
  template.hasResourceProperties("AWS::ECS::Cluster", { ClusterName: scheduledEcsClusterName("dev") });
  expect(Object.keys(template.findResources("AWS::ECS::TaskDefinition")).length).toBeGreaterThanOrEqual(2);
  expect(Object.keys(template.findResources("AWS::Scheduler::Schedule")).length).toBeGreaterThanOrEqual(2);
  template.hasResourceProperties("AWS::ECS::TaskDefinition", {
    Family: secondNames.family, Cpu: "512", Memory: "1024",
    RuntimePlatform: { OperatingSystemFamily: "LINUX", CpuArchitecture: "ARM64" },
  });
  const policies = Object.entries(template.findResources("AWS::IAM::Policy"));
  expect(policies.some(([id]) => id.includes("PeriodicMatcherTaskRole"))).toBe(false);
  const exampleRoleId = Object.keys(template.findResources("AWS::IAM::Role")).find((id) => id.includes("ExampleJobTaskRole"))!;
  const examplePolicies = policies.map(([, policy]) => policy)
    .filter((policy) => JSON.stringify(policy.Properties.Roles).includes(exampleRoleId));
  expect(examplePolicies).toHaveLength(1);
  expect(examplePolicies[0].Properties.PolicyDocument.Statement).toEqual([{
    Action: "s3:GetObject", Effect: "Allow", Resource: "arn:aws:s3:::example-bucket/input/*",
  }]);
  template.hasResourceProperties("AWS::Scheduler::Schedule", { Name: names.schedule });
  template.hasResourceProperties("AWS::Scheduler::Schedule", { Name: secondNames.schedule });
  const schedules = Object.values(template.findResources("AWS::Scheduler::Schedule"))
    .filter((schedule) => schedule.Properties.Target.EcsParameters);
  expect(schedules).toHaveLength(2);
  expect(schedules[0].Properties.Target.Arn).toEqual(schedules[1].Properties.Target.Arn);
  template.hasResourceProperties("AWS::SQS::Queue", { QueueName: secondNames.dlq });
  const exampleRules = Object.values(template.findResources("AWS::Events::Rule"))
    .filter((rule) => rule.Properties.Name?.includes("example-job"));
  expect(exampleRules).toHaveLength(3);
  const matcherExit = synthesizedRule(template, names.exitFailureRule).Properties.EventPattern;
  const exampleExit = synthesizedRule(template, secondNames.exitFailureRule).Properties.EventPattern;
  expect(exampleExit.detail.clusterArn).toEqual(matcherExit.detail.clusterArn);
  expect(exampleExit.detail.taskDefinitionArn).not.toEqual(matcherExit.detail.taskDefinitionArn);
  expect(exampleExit.detail.containers.name).toEqual(["example-job"]);
  expect(exampleExit.detail.exitCode).toBeUndefined();
  const matcherEvent = fixtureTaskEvent("dev", { containers: [{ name: "periodic-matcher", exitCode: 1 }] });
  const exampleEvent = { ...matcherEvent, detail: { ...matcherEvent.detail, taskDefinitionArn: `arn:aws:ecs:eu-central-1:123456789012:task-definition/${secondNames.family}:1`, containers: [{ name: "example-job", exitCode: 1 }] } };
  const matcherPattern = ecsTaskEventPattern("cluster", "matcher:", "periodic-matcher", "exit");
  const examplePattern = ecsTaskEventPattern("cluster", "example:", "example-job", "exit");
  expect(patternMatches(matcherPattern, { ...matcherEvent, detail: { ...matcherEvent.detail, clusterArn: "cluster", taskDefinitionArn: "matcher:1" } })).toBe(true);
  expect(patternMatches(examplePattern, { ...matcherEvent, detail: { ...matcherEvent.detail, clusterArn: "cluster", taskDefinitionArn: "matcher:1" } })).toBe(false);
  expect(patternMatches(examplePattern, { ...exampleEvent, detail: { ...exampleEvent.detail, clusterArn: "cluster", taskDefinitionArn: "example:1" } })).toBe(true);
  expect(patternMatches(matcherPattern, { ...exampleEvent, detail: { ...exampleEvent.detail, clusterArn: "cluster", taskDefinitionArn: "example:1" } })).toBe(false);
});

test.each([
  ["linux/amd64", cdk.aws_ecs.CpuArchitecture.X86_64],
  ["linux/arm64", cdk.aws_ecs.CpuArchitecture.ARM64],
] as const)("scheduled ECS platform %s maps to %s", (platform, architecture) => {
  expect(ecsCpuArchitectureForPlatform(platform)).toBe(architecture);
});

test("scheduled ECS platform conversion rejects unsupported platforms", () => {
  expect(() => ecsCpuArchitectureForPlatform("linux/s390x" as typeof PERIODIC_MATCHER_IMAGE.platform))
    .toThrow("Unsupported scheduled ECS image platform: linux/s390x");
});

test("ephemeral stages do not create the matcher or digest parameters", () => {
  const template = Template.fromStack(stacks("ephemeral").compute);
  template.resourceCountIs("AWS::ECS::TaskDefinition", 0);
  expect(template.toJSON().Parameters.PeriodicMatcherImageDigest).toBeUndefined();
});

// Offline check of the EventBridge pattern operators used here; not a substitute for AWS test-event-pattern.
function patternMatches(pattern: unknown, event: unknown): boolean {
  if (event === undefined) return false;
  if (Array.isArray(pattern)) {
    return pattern.some((choice) => Array.isArray(event)
      ? event.some((item) => patternMatches([choice], item))
      : choice && typeof choice === "object" && "prefix" in choice
        ? typeof event === "string" && event.startsWith(choice.prefix)
        : choice && typeof choice === "object" && "anything-but" in choice
          ? event !== undefined && event !== choice["anything-but"]
          : choice === event);
  }
  if (Array.isArray(event)) return event.some((item) => patternMatches(pattern, item));
  if (pattern && typeof pattern === "object" && event && typeof event === "object") {
    return Object.entries(pattern).every(([key, value]) => patternMatches(value, (event as Record<string, unknown>)[key]));
  }
  return false;
}

test("STOPPED fixtures distinguish successful, nonzero, startup and interruptions across revisions", () => {
  const cluster = "arn:aws:ecs:eu-central-1:123456789012:cluster/aura-historia-scheduled-prod";
  const family = "arn:aws:ecs:eu-central-1:123456789012:task-definition/aura-historia-periodic-matcher-prod:";
  const base = { source: "aws.ecs", "detail-type": "ECS Task State Change", detail: { clusterArn: cluster, taskDefinitionArn: `${family}42`, lastStatus: "STOPPED" } };
  const stopped = {
    source: ["aws.ecs"],
    "detail-type": ["ECS Task State Change"],
    detail: { clusterArn: [cluster], taskDefinitionArn: [{ prefix: family }], lastStatus: ["STOPPED"] },
  };
  const exit = matcherTaskEventPattern(cluster, family, "exit");
  const interruption = matcherTaskEventPattern(cluster, family, "interruption");
  const withContainer = (exitCode: number) => ({ ...base, detail: { ...base.detail, containers: [{ name: "periodic-matcher", exitCode }], stopCode: "EssentialContainerExited" } });
  const withoutExitCode = { ...base, detail: { ...base.detail, containers: [{ name: "periodic-matcher" }], stopCode: "TaskFailedToStart" } };
  const taskFailedToStart = { ...base, detail: { ...base.detail, stopCode: "TaskFailedToStart" } };
  expect(patternMatches(stopped, withContainer(0))).toBe(true);
  expect(patternMatches(stopped, withContainer(1))).toBe(true);
  expect(patternMatches(stopped, withoutExitCode)).toBe(true);
  expect(patternMatches(stopped, taskFailedToStart)).toBe(true);
  expect(patternMatches(stopped, { ...withContainer(0), detail: { ...withContainer(0).detail, taskDefinitionArn: `${family}41` } })).toBe(true);
  expect(patternMatches(stopped, { ...withContainer(0), detail: { ...withContainer(0).detail, clusterArn: "other" } })).toBe(false);
  expect(patternMatches(stopped, { ...withContainer(0), detail: { ...withContainer(0).detail, taskDefinitionArn: "arn:aws:ecs:eu-central-1:123456789012:task-definition/aura-historia-periodic-matcher-other:42" } })).toBe(false);
  expect(patternMatches(exit, withContainer(0))).toBe(false);
  expect(patternMatches(interruption, withContainer(0))).toBe(false);
  expect(patternMatches(exit, withContainer(1))).toBe(true);
  expect(patternMatches(exit, withContainer(137))).toBe(true);
  expect(patternMatches(exit, withoutExitCode)).toBe(false);
  expect(patternMatches(interruption, withoutExitCode)).toBe(true);
  expect(patternMatches(exit, taskFailedToStart)).toBe(false);
  expect(patternMatches(interruption, taskFailedToStart)).toBe(true);
  expect(patternMatches(exit, { ...withContainer(2), detail: { ...withContainer(2).detail, taskDefinitionArn: `${family}41` } })).toBe(true);
  expect(patternMatches(interruption, { ...base, detail: { ...base.detail, stopCode: "UserInitiated" } })).toBe(true);
  expect(patternMatches(exit, { ...withContainer(1), detail: { ...withContainer(1).detail, clusterArn: "other" } })).toBe(false);
  expect(patternMatches(exit, { ...withContainer(1), detail: { ...withContainer(1).detail, taskDefinitionArn: family.replace("matcher-prod:", "matcher-other:") + "42" } })).toBe(false);
});

test("failure patterns and production observability preserve their synthesized EventBridge contracts", () => {
  const exit = matcherTaskEventPattern("cluster", "family:", "exit");
  const interruption = matcherTaskEventPattern("cluster", "family:", "interruption");
  expect(exit).toEqual(ecsTaskEventPattern("cluster", "family:", "periodic-matcher", "exit"));
  expect(ecsTaskL2EventPattern("cluster", "family:", "periodic-matcher", "exit").detailType).toEqual(exit["detail-type"]);
  expect(exit["detail-type"]).toEqual(["ECS Task State Change"]);
  expect(exit.detail).toMatchObject({ clusterArn: ["cluster"], taskDefinitionArn: [{ prefix: "family:" }], containers: { name: ["periodic-matcher"], exitCode: [{ "anything-but": 0 }] } });
  expect(interruption.detail).not.toHaveProperty("containers");
  expect(interruption.detail).toMatchObject({ stopCode: expect.arrayContaining(["TaskFailedToStart", "UserInitiated"]) });

  const prod = stacks("prod");
  const observable = Template.fromStack(prod.observability!);
  observable.hasResourceProperties("AWS::CloudWatch::Alarm", { AlarmName: "prod-periodic-matcher-delivery-dlq-visible", Threshold: 1, TreatMissingData: "notBreaching" });
  const matcherFailureRules = Object.values(observable.findResources("AWS::Events::Rule")).filter((rule) => {
    const pattern = rule.Properties.EventPattern;
    return pattern?.source?.includes("aws.ecs") && pattern?.detail?.taskDefinitionArn !== undefined;
  });
  expect(matcherFailureRules).toHaveLength(2);
  const failureEvent = fixtureTaskEvent("prod", { containers: [{ name: "periodic-matcher", exitCode: 17 }], stopCode: "EssentialContainerExited" });
  const interruptionEvent = fixtureTaskEvent("prod", { stopCode: "TaskFailedToStart" });
  for (const rule of matcherFailureRules) {
    const pattern = rule.Properties.EventPattern;
    expectRawTaskPattern(pattern, "prod", [
      "clusterArn", "lastStatus", "taskDefinitionArn",
      ...(pattern.detail.containers ? ["containers"] : ["stopCode"]),
    ]);
    expect(JSON.stringify(pattern.detail.clusterArn)).toContain("aura-historia-scheduled-prod");
    const isExitFailure = pattern.detail.containers !== undefined;
    if (isExitFailure) {
      expect(pattern.detail.containers).toEqual({ name: ["periodic-matcher"], exitCode: [{ "anything-but": 0 }] });
      expect(pattern.detail).not.toHaveProperty("stopCode");
    } else {
      expect(pattern.detail.stopCode).toEqual(["TaskFailedToStart", "UserInitiated", "ServiceSchedulerInitiated", "SpotInterruption", "TerminationNotice"]);
      expect(pattern.detail).not.toHaveProperty("containers");
    }

    const transformer = rule.Properties.Targets[0].InputTransformer as Transformer;
    const event = isExitFailure ? failureEvent : interruptionEvent;
    const payload = renderInputTransformer(transformer, event);
    expect(payload).toEqual({
      classification: isExitFailure ? "application-container-nonzero" : "interruption",
      taskArn: event.detail.taskArn,
      taskDefinitionArn: event.detail.taskDefinitionArn,
      clusterArn: event.detail.clusterArn,
      lastStatus: "STOPPED",
      ...(isExitFailure ? { containerName: "periodic-matcher" } : { stopCode: "TaskFailedToStart" }),
    });
    expect(transformer.InputTemplate).not.toMatch(/containers|exitCode|reason|error|credential|password|secret/i);
  }
  expect(prod.compute.dependencies).not.toContain(prod.observability);
});
