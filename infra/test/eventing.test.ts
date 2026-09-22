import * as cdk from "aws-cdk-lib";
import { Template } from "aws-cdk-lib/assertions";
import { ApplicationEphemeralStack, createApplicationStacks } from "../src/application-stack";
import { STAGES, type StageName } from "../src/config";

type CloudFormationResource = {
  readonly DependsOn?: unknown;
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
  test("retains partner consumers while ProductListing activation is off by default", () => {
    const template = computeTemplate(stage);
    const templateJson = template.toJSON();
    const mappings = resources(template, "AWS::Lambda::EventSourceMapping");
    const rules = resources(template, "AWS::Events::Rule");
    const activation = { "Fn::If": ["ProductListingOpenSearchConsumerActivation", true, false] };
    const cdcRouterActivation = { "Fn::If": ["CdcRouterActivation", true, false] };

    expect(templateJson.Parameters.ProductListingOpenSearchConsumerEnabled).toMatchObject({
      Type: "String",
      Default: "false",
      AllowedValues: ["true", "false"],
    });
    expect(templateJson.Conditions.ProductListingOpenSearchConsumerActivation).toEqual({
      "Fn::Equals": [{ Ref: "ProductListingOpenSearchConsumerEnabled" }, "true"],
    });
    if (stage === "ephemeral") {
      expect(templateJson.Parameters.CdcRouterEnabled).toBeUndefined();
      expect(templateJson.Conditions.CdcRouterActivation).toBeUndefined();
    } else {
      expect(templateJson.Parameters.CdcRouterEnabled).toMatchObject({
        Type: "String",
        Default: "false",
        AllowedValues: ["true", "false"],
      });
      expect(templateJson.Conditions.CdcRouterActivation).toEqual({
        "Fn::Equals": [{ Ref: "CdcRouterEnabled" }, "true"],
      });
    }
    expect(mappings).toHaveLength(stage === "ephemeral" ? 2 : 3);
    expect(mappings.find((mapping) => mapping.Properties?.BatchSize === 10)?.Properties).toMatchObject({
      Enabled: activation,
      FunctionResponseTypes: ["ReportBatchItemFailures"],
      MaximumBatchingWindowInSeconds: 1,
    });
    const cdcRouterMapping = mappings.find((mapping) => mapping.Properties?.BatchSize === 100);
    if (stage === "ephemeral") {
      expect(cdcRouterMapping).toBeUndefined();
    } else {
      expect(cdcRouterMapping?.Properties).toMatchObject({ Enabled: cdcRouterActivation });
    }
    const productListingMapping = mappings.find((mapping) => mapping.Properties?.BatchSize === 1);
    expect(productListingMapping?.Properties).toMatchObject({
      Enabled: activation,
      FunctionResponseTypes: ["ReportBatchItemFailures"],
    });
    expect(JSON.stringify(productListingMapping?.Properties?.FunctionName)).toContain("ProductListingOpenSearchVersion");
    const stripeRule = rules.find((rule) => JSON.stringify(rule.Properties?.EventPattern).includes("customer.subscription.created"));
    const shopifyRule = rules.find((rule) => JSON.stringify(rule.Properties?.EventPattern).includes("X-Shopify-Topic"));
    expect(stripeRule?.Properties?.State).toEqual({ "Fn::If": ["ProductListingOpenSearchConsumerActivation", "ENABLED", "DISABLED"] });
    expect(shopifyRule?.Properties?.State).toEqual({ "Fn::If": ["ProductListingOpenSearchConsumerActivation", "ENABLED", "DISABLED"] });

    const fxRateSchedule = rules.find((rule) => rule.Properties?.ScheduleExpression === "cron(0 6,18 * * ? *)");
    expect(resources(template, "AWS::CloudFormation::CustomResource")).toHaveLength(0);
    expect(productListingMapping?.DependsOn).toBeUndefined();
    if (stage === "ephemeral") {
      expect(fxRateSchedule).toBeUndefined();
    } else {
      expect(fxRateSchedule?.Properties).toMatchObject({
        State: { "Fn::If": ["ProductListingOpenSearchConsumerActivation", "ENABLED", "DISABLED"] },
      });
      expect(JSON.stringify(template.toJSON())).not.toContain("fxrate-initial-snapshot-provider");
    }
  });
});
