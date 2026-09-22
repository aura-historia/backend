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
  test("retains partner consumers while ProductListing consumers are off by default", () => {
    const template = computeTemplate(stage);
    const templateJson = template.toJSON();
    const mappings = resources(template, "AWS::Lambda::EventSourceMapping");
    const rules = resources(template, "AWS::Events::Rule");
    const openSearchActivation = { "Fn::If": ["ProductListingOpenSearchConsumerActivation", true, false] };
    const normalizationActivation = { "Fn::If": ["ProductListingNormalizationConsumerActivation", true, false] };

    expect(templateJson.Parameters.ProductListingOpenSearchConsumerEnabled).toMatchObject({
      Type: "String",
      Default: "false",
      AllowedValues: ["true", "false"],
    });
    expect(templateJson.Conditions.ProductListingOpenSearchConsumerActivation).toEqual({
      "Fn::Equals": [{ Ref: "ProductListingOpenSearchConsumerEnabled" }, "true"],
    });
    expect(templateJson.Parameters.ProductListingNormalizationConsumerEnabled).toMatchObject({
      Type: "String",
      Default: "false",
      AllowedValues: ["true", "false"],
    });
    expect(templateJson.Conditions.ProductListingNormalizationConsumerActivation).toEqual({
      "Fn::Equals": [{ Ref: "ProductListingNormalizationConsumerEnabled" }, "true"],
    });
    expect(mappings).toHaveLength(stage === "ephemeral" ? 3 : 4);
    const shopifyMapping = mappings.find((mapping) =>
      JSON.stringify(mapping.Properties?.FunctionName).includes("LambdasShopifyLambda"),
    );
    expect(shopifyMapping?.Properties).toMatchObject({
      BatchSize: 10,
      Enabled: openSearchActivation,
      FunctionResponseTypes: ["ReportBatchItemFailures"],
      MaximumBatchingWindowInSeconds: 1,
    });
    const productListingMapping = mappings.find((mapping) =>
      JSON.stringify(mapping.Properties?.FunctionName).includes("ProductListingOpenSearchVersion"),
    );
    expect(productListingMapping?.Properties).toMatchObject({
      BatchSize: 1,
      Enabled: openSearchActivation,
      FunctionResponseTypes: ["ReportBatchItemFailures"],
    });
    const normalizationMapping = mappings.find((mapping) =>
      JSON.stringify(mapping.Properties?.FunctionName).includes("ProductListingNormalizationVersion"),
    );
    expect(normalizationMapping?.Properties).toMatchObject({
      BatchSize: 10,
      Enabled: normalizationActivation,
      FunctionResponseTypes: ["ReportBatchItemFailures"],
    });
    expect(JSON.stringify(normalizationMapping?.Properties?.EventSourceArn)).toContain(
      stage === "ephemeral"
        ? "WorkerQueuesProductListingNormalizationQueue"
        : `aura-worker-product-listing-normalization-${stage}`,
    );
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
