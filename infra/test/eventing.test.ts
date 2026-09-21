import * as cdk from "aws-cdk-lib";
import { Template } from "aws-cdk-lib/assertions";
import { ApplicationEphemeralStack, createApplicationStacks } from "../src/application-stack";
import { STAGES, type StageName } from "../src/config";

type CloudFormationResource = {
  readonly Properties?: Record<string, unknown>;
};

function computeTemplate(stage: StageName): Template {
  const app = new cdk.App({ analyticsReporting: false });
  if (stage === "ephemeral") {
    return Template.fromStack(new ApplicationEphemeralStack(app, "application-ephemeral", { stage }));
  }
  return Template.fromStack(createApplicationStacks(app, { stage }).compute);
}

function resources(template: Template, type: string): CloudFormationResource[] {
  return Object.values(template.findResources(type)) as CloudFormationResource[];
}

describe.each(STAGES)("%s compute eventing", (stage) => {
  test("always creates partner consumers and mappings", () => {
    const template = computeTemplate(stage);
    const mappings = resources(template, "AWS::Lambda::EventSourceMapping");
    const rules = resources(template, "AWS::Events::Rule");

    expect(mappings).toHaveLength(2);
    expect(mappings.find((mapping) => mapping.Properties?.BatchSize === 10)?.Properties).toMatchObject({
      FunctionResponseTypes: ["ReportBatchItemFailures"],
      MaximumBatchingWindowInSeconds: 1,
    });
    const productListingMapping = mappings.find((mapping) => mapping.Properties?.BatchSize === 1)?.Properties;
    expect(productListingMapping).toMatchObject({ FunctionResponseTypes: ["ReportBatchItemFailures"] });
    expect(JSON.stringify(productListingMapping?.FunctionName)).toContain("ProductListingOpenSearchVersion");
    expect(rules.some((rule) => JSON.stringify(rule.Properties?.EventPattern).includes("customer.subscription.created"))).toBe(true);
    expect(rules.some((rule) => JSON.stringify(rule.Properties?.EventPattern).includes("X-Shopify-Topic"))).toBe(true);

    const initialFxSnapshot = resources(template, "AWS::CloudFormation::CustomResource")
      .find((resource) => resource.Properties?.SourceEventId === `deployment:fxrate:initial:${stage}:v1`);
    if (stage === "ephemeral") {
      expect(initialFxSnapshot).toBeUndefined();
      expect(rules.some((rule) => rule.Properties?.ScheduleExpression === "cron(0 6,18 * * ? *)")).toBe(false);
    } else {
      expect(initialFxSnapshot).toBeDefined();
      expect(rules.some((rule) => rule.Properties?.ScheduleExpression === "cron(0 6,18 * * ? *)")).toBe(true);
    }
  });
});
