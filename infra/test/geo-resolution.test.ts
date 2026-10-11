import * as cdk from "aws-cdk-lib";
import { Template } from "aws-cdk-lib/assertions";
import { createApplicationStacks } from "../src/application-stack";
import { STAGES } from "../src/config";

describe.each(STAGES)("%s durable geographic resolution", (stage) => {
  test("has bounded paid work and shares the hourly backend cleanup schedule for recovery", () => {
    const app = new cdk.App({ analyticsReporting: false });
    const stacks = createApplicationStacks(app, { stage });
    const compute = Template.fromStack(stacks.compute);
    const data = Template.fromStack(stacks.data);
    compute.hasResourceProperties("AWS::Lambda::Function", {
      FunctionName: `geo-resolution-lambda-${stage}`, Timeout: 45, MemorySize: 256,
      ReservedConcurrentExecutions: 2,
      Environment: { Variables: { GEO_RESOLUTION_ENABLED: "false", POSTGRES_MAX_CONNECTIONS: "1" } },
    });
    const mappings = Object.values(compute.findResources("AWS::Lambda::EventSourceMapping"));
    const mapping = mappings.find((m) => JSON.stringify(m.Properties.FunctionName).includes("GeoResolutionVersion"));
    expect(mapping?.Properties).toMatchObject({ Enabled: true, BatchSize: 1, ScalingConfig: { MaximumConcurrency: 2 }, FunctionResponseTypes: ["ReportBatchItemFailures"] });
    expect(mapping?.DependsOn).toBeDefined();
    expect(JSON.stringify(mapping?.Properties.EventSourceArn)).toContain(`aura-worker-geo-resolution-${stage}`);
    data.hasResourceProperties("AWS::SQS::Queue", { QueueName: `aura-worker-geo-resolution-${stage}`, VisibilityTimeout: 300, RedrivePolicy: { maxReceiveCount: 5 } });
    data.hasResourceProperties("AWS::SQS::Queue", { QueueName: `aura-worker-geo-resolution-dlq-${stage}`, MessageRetentionPeriod: 1209600 });
    const schedules = Object.values(compute.findResources("AWS::Scheduler::Schedule"));
    expect(schedules.some((s) => JSON.stringify(s.Properties.Target.Arn).includes("GeoResolutionVersion"))).toBe(false);
    const cleanup = schedules.find((s) => s.Properties.Name === `aura-historia-expired-credential-cleanup-${stage}`);
    expect(cleanup?.Properties).toMatchObject({ ScheduleExpression: "cron(0 * * * ? *)", State: "ENABLED", Target: { DeadLetterConfig: { Arn: expect.anything() }, RetryPolicy: { MaximumRetryAttempts: 3 } } });
    expect(JSON.stringify(cleanup?.Properties.Target.Arn)).toContain("BackendCleanupVersion");
    compute.hasResourceProperties("AWS::Lambda::Function", {
      FunctionName: `backend-cleanup-lambda-${stage}`, Timeout: 10,
      Environment: { Variables: { EXPIRY_CLEANUP_BATCH_SIZE: "100" } },
    });
    const policies = Object.values(compute.findResources("AWS::IAM::Policy"));
    const policy = policies.find((p) => JSON.stringify(p.Properties.Roles).includes("GeoResolutionLambdaServiceRole"));
    expect(policy?.Properties.PolicyDocument.Statement).toContainEqual({ Action: "geo-places:Geocode", Effect: "Allow", Resource: "*", Condition: { StringEquals: { "aws:RequestedRegion": "eu-central-1" } } });
    expect(JSON.stringify(policy?.Properties)).not.toMatch(/geo-places:\*|sqs:SendMessage|es:ESHttp|ses:SendEmail/);
    const cleanupPolicy = policies.find((p) => JSON.stringify(p.Properties.Roles).includes("BackendCleanupLambdaServiceRole"));
    expect(cleanupPolicy).toBeDefined();
    expect(JSON.stringify(cleanupPolicy?.Properties)).not.toMatch(/geo-places:|sqs:SendMessage/);
    const schedulerPolicy = policies.find((p) => JSON.stringify(p.Properties.Roles).includes("MaintenanceSchedulerExecutionRole"));
    expect(schedulerPolicy).toBeDefined();
    expect(JSON.stringify(schedulerPolicy?.Properties)).not.toContain("GeoResolutionVersion");
    const task = Object.values(data.findResources("AWS::DMS::ReplicationTask"))[0];
    const tables = JSON.parse(task.Properties.TableMappings).rules.filter((r: Record<string, string>) => r["rule-type"] === "selection").map((r: Record<string, Record<string, string>>) => r["object-locator"]["table-name"]);
    expect(tables).toContain("geo_resolution_wakeups");
    for (const privateTable of ["geo_resolutions", "geo_resolution_bindings", "geo_resolution_provider_controls", "party_locations", "party_location_create_receipts"]) { expect(tables).not.toContain(privateTable); }
  });
});
