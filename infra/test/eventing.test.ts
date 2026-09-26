import * as cdk from "aws-cdk-lib";
import { Template } from "aws-cdk-lib/assertions";
import { ApplicationEphemeralStack, createApplicationStacks } from "../src/application-stack";
import { STAGES, type StageName } from "../src/config";

type Resource = { Properties: Record<string, any> };

function templates(stage: StageName) {
  const app = new cdk.App({ analyticsReporting: false });
  if (stage === "ephemeral") {
    return { compute: Template.fromStack(new ApplicationEphemeralStack(app, "application-ephemeral", { stage })) };
  }
  const stacks = createApplicationStacks(app, { stage });
  return { compute: Template.fromStack(stacks.compute), initialize: Template.fromStack(stacks.initialization!) };
}

function resources(template: Template, type: string): Resource[] {
  return Object.values(template.findResources(type)) as Resource[];
}

describe.each(STAGES)("%s native eventing", (stage) => {
  test("activates partner and SQS consumers by default; only the external DMS router requires approval", () => {
    const { compute } = templates(stage);
    const json = compute.toJSON();
    expect(Object.keys(json.Parameters)).toEqual(stage === "ephemeral" ? ["CommitSHA"] : ["CommitSHA", "CdcRouterEnabled"]);
    expect(json.Conditions ?? {}).toEqual(stage === "ephemeral" ? {} : {
      CdcRouterActivation: { "Fn::Equals": [{ Ref: "CdcRouterEnabled" }, "true"] },
    });
    if (stage !== "ephemeral") {
      expect(json.Parameters.CdcRouterEnabled).toMatchObject({ Default: "false", AllowedValues: ["true", "false"] });
    }

    const mappings = resources(compute, "AWS::Lambda::EventSourceMapping");
    expect(mappings).toHaveLength(stage === "ephemeral" ? 11 : 12);
    const router = mappings.find((mapping) => mapping.Properties.BatchSize === 100);
    if (stage === "ephemeral") {
      expect(router).toBeUndefined();
    } else {
      expect(router?.Properties.Enabled).toEqual({ "Fn::If": ["CdcRouterActivation", true, false] });
    }
    for (const mapping of mappings.filter((candidate) => candidate !== router)) {
      expect(mapping.Properties.Enabled === undefined || mapping.Properties.Enabled === true).toBe(true);
      expect(mapping.Properties.FunctionResponseTypes).toEqual(["ReportBatchItemFailures"]);
    }
    const normalization = mappings.find((mapping) => JSON.stringify(mapping.Properties.FunctionName).includes("ProductListingNormalizationVersion"));
    expect(normalization?.Properties.BatchSize).toBe(10);
    expect(JSON.stringify(normalization?.Properties.EventSourceArn)).toContain(
      stage === "ephemeral" ? "WorkerQueuesProductListingNormalizationQueue" : `aura-worker-product-listing-normalization-${stage}`,
    );
    const shopify = mappings.find((mapping) => JSON.stringify(mapping.Properties.FunctionName).includes("ShopifyLambda"));
    expect(shopify?.Properties).toMatchObject({ BatchSize: 10, MaximumBatchingWindowInSeconds: 1 });

    const rules = resources(compute, "AWS::Events::Rule");
    for (const marker of ["customer.subscription.created", "X-Shopify-Topic"]) {
      const rule = rules.find((candidate) => JSON.stringify(candidate.Properties.EventPattern).includes(marker));
      expect(rule).toBeDefined();
      expect(rule?.Properties.State ?? "ENABLED").toBe("ENABLED");
    }
    expect(resources(compute, "AWS::CloudFormation::CustomResource")).toHaveLength(0);
  });

  test("schedules cleanup and the initialization FX version for real stages only", () => {
    const { compute, initialize } = templates(stage);
    const schedules = resources(compute, "AWS::Scheduler::Schedule");
    if (stage === "ephemeral") {
      expect(schedules).toHaveLength(0);
      expect(JSON.stringify(compute.toJSON())).not.toContain("fxrate-lambda-ephemeral");
      return;
    }
    expect(schedules).toHaveLength(2);
    const fx = schedules.find((schedule) => schedule.Properties.ScheduleExpression === "cron(0 6,18 * * ? *)");
    const cleanup = schedules.find((schedule) => schedule.Properties.ScheduleExpression === "cron(0 * * * ? *)");
    for (const schedule of [fx, cleanup]) {
      expect(schedule?.Properties).toMatchObject({
        State: "ENABLED",
        ScheduleExpressionTimezone: "UTC",
        FlexibleTimeWindow: { Mode: "OFF" },
        Target: { DeadLetterConfig: { Arn: expect.anything() }, RetryPolicy: { MaximumEventAgeInSeconds: 3600, MaximumRetryAttempts: 3 } },
      });
    }
    expect(cleanup?.Properties.Target.Input).toBe('{"schedule":"expired-credential-cleanup"}');
    expect(JSON.stringify(cleanup?.Properties.Target.Arn)).toContain("BackendCleanupVersion");
    expect(fx?.Properties.Target.Input).toBe(
      '{"version":"0","id":"fxrate:<aws.scheduler.scheduled-time>","detail-type":"Scheduled Event","source":"aura-historia.scheduler","account":"000000000000","time":"<aws.scheduler.scheduled-time>","region":"eu-central-1","resources":["<aws.scheduler.schedule-arn>"],"detail":{}}',
    );
    const importedVersion = fx?.Properties.Target.Arn;
    expect(importedVersion).toEqual({ "Fn::ImportValue": expect.any(String) });
    expect(Object.values(initialize!.toJSON().Outputs).some((output: any) =>
      output.Export?.Name === importedVersion["Fn::ImportValue"] && JSON.stringify(output.Value).includes("FxRateSyncVersion"),
    )).toBe(true);
    const schedulerPolicy = resources(compute, "AWS::IAM::Policy").find((policy) =>
      JSON.stringify(policy.Properties).includes("lambda:InvokeFunction") && JSON.stringify(policy.Properties).includes("sqs:SendMessage"),
    );
    expect(JSON.stringify(schedulerPolicy?.Properties)).toContain("BackendCleanupVersion");
    expect(JSON.stringify(schedulerPolicy?.Properties)).toContain(importedVersion["Fn::ImportValue"]);
    expect(JSON.stringify(compute.toJSON())).not.toContain(`/fxratesapi/${stage}/api-token`);
    expect(JSON.stringify(initialize!.toJSON())).toContain(`/fxratesapi/${stage}/api-token`);
  });
});
