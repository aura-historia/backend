import * as cdk from "aws-cdk-lib";
import { Template } from "aws-cdk-lib/assertions";
import { ApplicationEphemeralStack, createApplicationStacks } from "../src/application-stack";
import { STAGES, type StageName } from "../src/config";

type CloudFormationResource = {
  readonly Properties?: Record<string, unknown>;
};

function templatesFor(stage: StageName): Template[] {
  if (stage === "ephemeral") {
    const app = new cdk.App({ analyticsReporting: false });
    return [Template.fromStack(new ApplicationEphemeralStack(app, "application-ephemeral", { stage }))];
  }

  const app = new cdk.App({ analyticsReporting: false });
  const stacks = createApplicationStacks(app, { stage });
  const stackList: cdk.Stack[] = [];
  for (const stack of [stacks.network, stacks.data, stacks.initialization, stacks.compute, stacks.api, stacks.observability]) {
    if (stack !== undefined) {
      stackList.push(stack);
    }
  }
  return stackList.map((stack) => Template.fromStack(stack));
}

function resources(template: Template, type: string): CloudFormationResource[] {
  return Object.values(template.findResources(type)) as CloudFormationResource[];
}

describe.each(STAGES)("%s Lambda concurrency guard", (stage) => {
  test("rejects all unapproved Lambda and SQS concurrency controls", () => {
    for (const template of templatesFor(stage)) {
      for (const resource of resources(template, "AWS::Lambda::Function")) {
        expect(resource.Properties?.ReservedConcurrentExecutions).toBeUndefined();
      }

      for (const resource of resources(template, "AWS::Lambda::Alias")) {
        expect(resource.Properties?.ProvisionedConcurrencyConfig).toBeUndefined();
      }

      for (const resource of resources(template, "AWS::Lambda::EventSourceMapping")) {
        expect(resource.Properties?.ProvisionedPollerConfig).toBeUndefined();
        expect(resource.Properties?.ScalingConfig).toBeUndefined();
      }
    }
  });
});
