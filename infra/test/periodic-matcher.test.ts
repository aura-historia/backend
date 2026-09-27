import * as cdk from "aws-cdk-lib";
import { Match, Template } from "aws-cdk-lib/assertions";
import { createApplicationStacks } from "../src/application-stack";
import { matcherTaskEventPattern } from "../src/periodic-matcher-config";

function stacks(stage: "dev" | "prod" | "ephemeral") {
  return createApplicationStacks(new cdk.App({ analyticsReporting: false }), { stage });
}

describe.each(["dev", "prod"] as const)("%s periodic matcher", (stage) => {
  test("requires immutable digest, starts disabled, runs one private Fargate task daily", () => {
    const stack = stacks(stage);
    const template = Template.fromStack(stack.compute);
    expect(template.toJSON().Parameters.PeriodicMatcherImageDigest).toMatchObject({ Type: "String", AllowedPattern: "^sha256:[0-9a-f]{64}$" });
    expect(template.toJSON().Parameters.PeriodicMatcherImageDigest.Default).toBeUndefined();
    expect(template.toJSON().Parameters.PeriodicMatcherEnabled).toMatchObject({ Default: "false", AllowedValues: ["true", "false"] });
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
    const task = Object.values(template.findResources("AWS::ECS::TaskDefinition"))[0].Properties;
    expect(task.ContainerDefinitions).toHaveLength(1);
    expect(JSON.stringify(task.ContainerDefinitions[0].Image)).toContain("PeriodicMatcherImageDigest");
    expect(JSON.stringify(task.ContainerDefinitions[0].Image)).toContain("aura-historia-periodic-matcher");
    expect(task.ContainerDefinitions[0].Secrets).toHaveLength(5);
    expect(task.ContainerDefinitions[0].Environment.map((entry: { Name: string }) => entry.Name)).not.toEqual(expect.arrayContaining(["POSTGRES_PASSWORD", "OPENSEARCH_PASSWORD", "AURA_HISTORIA_GOOGLE_ADC_CREDENTIALS_JSON"]));
    expect(JSON.stringify(task.ContainerDefinitions[0].Secrets)).toContain(`/opensearch/${stage}/reader/password`);
    expect(JSON.stringify(task.ContainerDefinitions[0].Secrets)).toContain(`/secrets/${stage}/google-application-credentials`);
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
    const schedule = Object.values(template.findResources("AWS::Scheduler::Schedule")).find((resource) => resource.Properties.Name === `search-filter-periodic-match-${stage}`)!;
    expect(schedule.DependsOn).toContain(schedulerPolicyId);
    expect(JSON.stringify(template.toJSON())).not.toContain("PeriodicMatcherReaderPasswordParameter");
    template.resourceCountIs("AWS::Logs::ResourcePolicy", 1);
    expect(Object.keys(template.findResources("AWS::CloudFormation::CustomResource")).filter((key) => key.includes("PeriodicMatcher"))).toHaveLength(0);
    template.hasResourceProperties("AWS::Scheduler::Schedule", {
      ScheduleExpression: "cron(0 15 * * ? *)", ScheduleExpressionTimezone: "UTC", FlexibleTimeWindow: { Mode: "OFF" },
      Target: Match.objectLike({ EcsParameters: Match.objectLike({ TaskCount: 1, LaunchType: "FARGATE", PlatformVersion: "1.4.0", NetworkConfiguration: Match.objectLike({ AwsvpcConfiguration: Match.objectLike({ AssignPublicIp: "DISABLED" }) }) }), RetryPolicy: { MaximumEventAgeInSeconds: 3600, MaximumRetryAttempts: 2 } }),
    });
    template.resourceCountIs("AWS::ECS::Service", 0);
    const lifecycleRules = Object.values(template.findResources("AWS::Events::Rule")).filter((rule) => String(rule.Properties.Name).includes("periodic-matcher"));
    expect(lifecycleRules).toHaveLength(3);
    for (const rule of lifecycleRules) {
      const transformer = rule.Properties.Targets[0].InputTransformer;
      const payload = JSON.parse(transformer.InputTemplate.replace(/<[^>]+>/g, '"safe"'));
      expect(Object.keys(payload)).toEqual(expect.arrayContaining(["timestamp", "classification", "taskArn", "taskDefinitionArn", "clusterArn", "status"]));
      expect(JSON.stringify(payload)).not.toMatch(/containers|exitCode|reason|error|message|credential|password|secret/i);
      expect(Object.keys(transformer.InputPathsMap)).not.toEqual(expect.arrayContaining(["containers", "reason", "error", "message"]));
    }
    template.hasResourceProperties("AWS::SQS::Queue", { QueueName: `aura-historia-periodic-matcher-delivery-${stage}`, SqsManagedSseEnabled: true, MessageRetentionPeriod: 1209600 });
    template.resourceCountIs("AWS::ECR::Repository", 0);
    expect(stack.compute.periodicMatcher).toBeDefined();
    expect(JSON.stringify(Template.fromStack(stack.network!).toJSON())).toContain("starport-layer-bucket/*");
  });
});

test("ephemeral stages do not create the matcher or digest parameters", () => {
  const template = Template.fromStack(stacks("ephemeral").compute);
  template.resourceCountIs("AWS::ECS::TaskDefinition", 0);
  expect(template.toJSON().Parameters.PeriodicMatcherImageDigest).toBeUndefined();
});

// Offline check of the EventBridge pattern operators used here; not a substitute for AWS test-event-pattern.
function patternMatches(pattern: unknown, event: unknown): boolean {
  if (Array.isArray(pattern)) {
    return pattern.some((choice) => Array.isArray(event)
      ? event.some((item) => patternMatches([choice], item))
      : choice && typeof choice === "object" && "prefix" in choice
        ? typeof event === "string" && event.startsWith(choice.prefix)
        : choice && typeof choice === "object" && "anything-but" in choice
          ? event !== choice["anything-but"]
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
  const exit = matcherTaskEventPattern(cluster, family, "exit");
  const interruption = matcherTaskEventPattern(cluster, family, "interruption");
  const withContainer = (exitCode: number) => ({ ...base, detail: { ...base.detail, containers: [{ name: "periodic-matcher", exitCode }], stopCode: "EssentialContainerExited" } });
  expect(patternMatches(exit, withContainer(0))).toBe(false);
  expect(patternMatches(interruption, withContainer(0))).toBe(false);
  expect(patternMatches(exit, withContainer(1))).toBe(true);
  expect(patternMatches(exit, withContainer(137))).toBe(true);
  expect(patternMatches(exit, { ...withContainer(2), detail: { ...withContainer(2).detail, taskDefinitionArn: `${family}41` } })).toBe(true);
  expect(patternMatches(interruption, { ...base, detail: { ...base.detail, stopCode: "TaskFailedToStart" } })).toBe(true);
  expect(patternMatches(interruption, { ...base, detail: { ...base.detail, stopCode: "UserInitiated" } })).toBe(true);
  expect(patternMatches(exit, { ...withContainer(1), detail: { ...withContainer(1).detail, clusterArn: "other" } })).toBe(false);
  expect(patternMatches(exit, { ...withContainer(1), detail: { ...withContainer(1).detail, taskDefinitionArn: family.replace("matcher-prod:", "matcher-other:") + "42" } })).toBe(false);
});

test("failure patterns select named nonzero container or interruption without optional exit fields", () => {
  const exit = matcherTaskEventPattern("cluster", "family:", "exit");
  const interruption = matcherTaskEventPattern("cluster", "family:", "interruption");
  expect(exit.detail).toMatchObject({ clusterArn: ["cluster"], taskDefinitionArn: [{ prefix: "family:" }], containers: { name: ["periodic-matcher"], exitCode: [{ "anything-but": 0 }] } });
  expect(interruption.detail).not.toHaveProperty("containers");
  expect(interruption.detail).toMatchObject({ stopCode: expect.arrayContaining(["TaskFailedToStart", "UserInitiated"]) });
  const prod = stacks("prod");
  const observable = Template.fromStack(prod.observability!);
  observable.hasResourceProperties("AWS::CloudWatch::Alarm", { AlarmName: "prod-periodic-matcher-delivery-dlq-visible", Threshold: 1, TreatMissingData: "notBreaching" });
  const matcherFailureRules = Object.values(observable.findResources("AWS::Events::Rule")).filter((rule) => JSON.stringify(rule).includes("PeriodicMatcher") || JSON.stringify(rule).includes("periodic-matcher-prod:"));
  expect(matcherFailureRules).toHaveLength(2);
  for (const rule of matcherFailureRules) {
    const transformer = rule.Properties.Targets[0].InputTransformer;
    const payload = JSON.parse(transformer.InputTemplate.replace(/<[^>]+>/g, '"safe"'));
    expect(Object.keys(payload)).toEqual(expect.arrayContaining(["classification", "taskArn", "taskDefinitionArn", "clusterArn", "lastStatus"]));
    expect(JSON.stringify(payload)).not.toMatch(/detail|containers|exitCode|reason|error|message|credential|password|secret/i);
  }
  expect(prod.compute.dependencies).not.toContain(prod.observability);
});
