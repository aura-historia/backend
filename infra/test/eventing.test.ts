import * as cdk from "aws-cdk-lib";
import { Template } from "aws-cdk-lib/assertions";
import { ApplicationEphemeralStack, createApplicationStacks } from "../src/application-stack";
import { STAGES, type StageName } from "../src/config";

type Resource = { Properties: Record<string, any> };

function templates(stage: StageName, sha?: string) {
  const app = new cdk.App({ analyticsReporting: false });
  if (stage === "ephemeral") {
    return { compute: Template.fromStack(new ApplicationEphemeralStack(app, "application-ephemeral", { stage })) };
  }
  const stacks = createApplicationStacks(app, { stage });
  if (sha) {
    (stacks.initialization!.node.findChild("CommitSHA") as cdk.CfnParameter).default = sha;
    (stacks.compute.node.findChild("CommitSHA") as cdk.CfnParameter).default = sha;
  }
  return {
    compute: Template.fromStack(stacks.compute),
    initialize: Template.fromStack(stacks.initialization!),
  };
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

  test("schedules cleanup version and the unqualified initialization FX function for real stages only", () => {
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
    const fxArn = fx?.Properties.Target.Arn;
    expect(JSON.stringify(fxArn)).toContain(`:function:fxrate-lambda-${stage}`);
    expect(JSON.stringify(fxArn)).not.toContain("Fn::ImportValue");
    expect(JSON.stringify(initialize!.toJSON().Outputs ?? {})).not.toContain("FxRateSyncVersion");
    initialize!.resourceCountIs("AWS::Lambda::Version", 0);
    const schedulerPolicy = resources(compute, "AWS::IAM::Policy").find((policy) =>
      JSON.stringify(policy.Properties).includes("lambda:InvokeFunction") && JSON.stringify(policy.Properties).includes("sqs:SendMessage"),
    );
    expect(JSON.stringify(schedulerPolicy?.Properties)).toContain("BackendCleanupVersion");
    expect(schedulerPolicy?.Properties.PolicyDocument.Statement).toEqual(expect.arrayContaining([
      expect.objectContaining({ Action: "lambda:InvokeFunction", Resource: expect.arrayContaining([fxArn]) }),
    ]));
    expect(JSON.stringify(schedulerPolicy?.Properties.PolicyDocument.Statement)).toContain(`:function:fxrate-lambda-${stage}`);
    expect(JSON.stringify(compute.toJSON())).not.toContain("FxRateSyncVersion");
    expect(JSON.stringify(compute.toJSON())).not.toContain(`/fxratesapi/${stage}/api-token`);
    expect(JSON.stringify(initialize!.toJSON())).toContain(`/fxratesapi/${stage}/api-token`);
  });
});


describe.each(["dev", "prod"] as const)("%s FX deployment boundary", (stage) => {
  test("has one initialization FX function, no FX version export/import, and a stable target across SHAs", () => {
    const first = templates(stage, "first-sha");
    const second = templates(stage, "second-sha");
    const fxFunctions = [first.initialize!, first.compute].flatMap((template) =>
      resources(template, "AWS::Lambda::Function").filter((resource) =>
        resource.Properties.FunctionName === `fxrate-lambda-${stage}`,
      ),
    );
    expect(fxFunctions).toHaveLength(1);
    expect(JSON.stringify(fxFunctions[0].Properties.Code)).toContain("CommitSHA");
    expect(first.initialize!.toJSON().Parameters.CommitSHA.Default).toBe("first-sha");
    expect(second.initialize!.toJSON().Parameters.CommitSHA.Default).toBe("second-sha");
    first.initialize!.resourceCountIs("AWS::Lambda::Version", 0);
    expect(JSON.stringify(first.initialize!.toJSON().Outputs ?? {})).not.toContain("FxRateSync");

    const fxTarget = (template: Template) => resources(template, "AWS::Scheduler::Schedule")
      .find((schedule) => schedule.Properties.ScheduleExpression === "cron(0 6,18 * * ? *)")?.Properties.Target.Arn;
    const fxArn = fxTarget(first.compute);
    expect(JSON.stringify(fxArn)).toContain(`:function:fxrate-lambda-${stage}`);
    expect(fxTarget(second.compute)).toEqual(fxArn);
    expect(JSON.stringify(fxArn)).not.toMatch(/CommitSHA|Fn::ImportValue|FxRateSyncVersion/);
    const fxInvokeResources = (template: Template) => resources(template, "AWS::IAM::Policy")
      .flatMap((policy) => policy.Properties.PolicyDocument.Statement)
      .find((statement: any) => JSON.stringify(statement.Resource).includes(`fxrate-lambda-${stage}`))?.Resource;
    expect(fxInvokeResources(first.compute)).toEqual(expect.arrayContaining([fxArn]));
    expect(JSON.stringify(fxInvokeResources(first.compute))).toContain(`:function:fxrate-lambda-${stage}`);
    expect(fxInvokeResources(second.compute)).toEqual(fxInvokeResources(first.compute));
    expect(resources(first.compute, "AWS::Lambda::Version").length).toBeGreaterThan(0);
    expect(JSON.stringify(first.compute.toJSON())).not.toContain("FxRateSyncVersion");
    expect(JSON.stringify(second.compute.toJSON())).not.toContain("FxRateSyncVersion");
  });

  test("orders compute after initialization without a reverse dependency cycle", () => {
    const app = new cdk.App({ analyticsReporting: false });
    const stacks = createApplicationStacks(app, { stage });
    expect(stacks.compute.dependencies).toContain(stacks.initialization);
    const predecessors = new Set<cdk.Stack>();
    const visit = (stack: cdk.Stack): void => {
      if (predecessors.has(stack)) return;
      predecessors.add(stack);
      stack.dependencies.forEach(visit);
    };
    visit(stacks.initialization!);
    expect(predecessors.has(stacks.compute)).toBe(false);
    Template.fromStack(stacks.compute);
    Template.fromStack(stacks.initialization!);
  });
});
