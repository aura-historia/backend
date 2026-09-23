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
  test("retains partner consumers while ProductListing and CDC consumers are off by default", () => {
    const template = computeTemplate(stage);
    const templateJson = template.toJSON();
    const mappings = resources(template, "AWS::Lambda::EventSourceMapping");
    const rules = resources(template, "AWS::Events::Rule");
    const openSearchActivation = { "Fn::If": ["ProductListingOpenSearchConsumerActivation", true, false] };
    const normalizationActivation = { "Fn::If": ["ProductListingNormalizationConsumerActivation", true, false] };
    const contentAssessmentActivation = { "Fn::If": ["ProductContentAssessmentConsumerActivation", true, false] };
    const embeddingActivation = { "Fn::If": ["ProductEmbeddingConsumerActivation", true, false] };
    const projectionActivation = { "Fn::If": ["SearchFilterProjectionConsumerActivation", true, false] };
    const percolatorActivation = { "Fn::If": ["SearchFilterPercolatorConsumerActivation", true, false] };
    const matchNotificationActivation = { "Fn::If": ["SearchFilterMatchNotificationConsumerActivation", true, false] };
    const watchlistNotificationActivation = { "Fn::If": ["WatchlistNotificationConsumerActivation", true, false] };
    const notificationDeliveryActivation = { "Fn::If": ["NotificationDeliveryConsumerActivation", true, false] };
    const cdcRouterActivation = { "Fn::If": ["CdcRouterActivation", true, false] };

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
    expect(templateJson.Parameters.ProductContentAssessmentConsumerEnabled).toMatchObject({
      Type: "String",
      Default: "false",
      AllowedValues: ["true", "false"],
    });
    expect(templateJson.Conditions.ProductContentAssessmentConsumerActivation).toEqual({
      "Fn::Equals": [{ Ref: "ProductContentAssessmentConsumerEnabled" }, "true"],
    });
    expect(templateJson.Parameters.ProductEmbeddingConsumerEnabled).toMatchObject({
      Type: "String",
      Default: "false",
      AllowedValues: ["true", "false"],
    });
    expect(templateJson.Conditions.ProductEmbeddingConsumerActivation).toEqual({
      "Fn::Equals": [{ Ref: "ProductEmbeddingConsumerEnabled" }, "true"],
    });
    expect(templateJson.Parameters.SearchFilterProjectionConsumerEnabled).toMatchObject({
      Type: "String",
      Default: "false",
      AllowedValues: ["true", "false"],
    });
    expect(templateJson.Conditions.SearchFilterProjectionConsumerActivation).toEqual({
      "Fn::Equals": [{ Ref: "SearchFilterProjectionConsumerEnabled" }, "true"],
    });
    expect(templateJson.Parameters.SearchFilterPercolatorConsumerEnabled).toMatchObject({
      Type: "String",
      Default: "false",
      AllowedValues: ["true", "false"],
    });
    expect(templateJson.Conditions.SearchFilterPercolatorConsumerActivation).toEqual({
      "Fn::Equals": [{ Ref: "SearchFilterPercolatorConsumerEnabled" }, "true"],
    });
    for (const [parameter, condition] of [
      ["SearchFilterMatchNotificationConsumerEnabled", "SearchFilterMatchNotificationConsumerActivation"],
      ["WatchlistNotificationConsumerEnabled", "WatchlistNotificationConsumerActivation"],
      ["NotificationDeliveryConsumerEnabled", "NotificationDeliveryConsumerActivation"],
    ]) {
      expect(templateJson.Parameters[parameter]).toMatchObject({
        Type: "String",
        Default: "false",
        AllowedValues: ["true", "false"],
      });
      expect(templateJson.Conditions[condition]).toEqual({
        "Fn::Equals": [{ Ref: parameter }, "true"],
      });
    }
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

    expect(mappings).toHaveLength(stage === "ephemeral" ? 10 : 11);
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
    const contentAssessmentMapping = mappings.find((mapping) =>
      JSON.stringify(mapping.Properties?.FunctionName).includes("ProductContentAssessmentVersion"),
    );
    expect(contentAssessmentMapping?.Properties).toMatchObject({
      BatchSize: 1,
      Enabled: contentAssessmentActivation,
      FunctionResponseTypes: ["ReportBatchItemFailures"],
    });
    expect(JSON.stringify(contentAssessmentMapping?.Properties?.EventSourceArn)).toContain(
      stage === "ephemeral"
        ? "WorkerQueuesProductContentAssessmentQueue"
        : `aura-worker-product-content-assessment-${stage}`,
    );
    const embeddingMapping = mappings.find((mapping) =>
      JSON.stringify(mapping.Properties?.FunctionName).includes("ProductEmbeddingVersion"),
    );
    expect(embeddingMapping?.Properties).toMatchObject({
      BatchSize: 1,
      Enabled: embeddingActivation,
      FunctionResponseTypes: ["ReportBatchItemFailures"],
    });
    expect(JSON.stringify(embeddingMapping?.Properties?.EventSourceArn)).toContain(
      stage === "ephemeral"
        ? "WorkerQueuesProductEmbeddingQueue"
        : `aura-worker-product-embedding-${stage}`,
    );
    const projectionMapping = mappings.find((mapping) =>
      JSON.stringify(mapping.Properties?.FunctionName).includes("SearchFilterProjectionVersion"),
    );
    expect(projectionMapping?.Properties).toMatchObject({
      BatchSize: 1,
      Enabled: projectionActivation,
      FunctionResponseTypes: ["ReportBatchItemFailures"],
    });
    expect(JSON.stringify(projectionMapping?.Properties?.EventSourceArn)).toContain(
      stage === "ephemeral"
        ? "WorkerQueuesSearchFilterProjectionQueue"
        : `aura-worker-search-filter-projection-${stage}`,
    );
    const notificationDeliveryMapping = mappings.find((mapping) =>
      JSON.stringify(mapping.Properties?.FunctionName).includes("NotificationDeliveryVersion"),
    );
    expect(notificationDeliveryMapping?.Properties).toMatchObject({
      BatchSize: 1,
      Enabled: notificationDeliveryActivation,
      FunctionResponseTypes: ["ReportBatchItemFailures"],
    });
    expect(JSON.stringify(notificationDeliveryMapping?.Properties?.EventSourceArn)).toContain(
      stage === "ephemeral"
        ? "WorkerQueuesNotificationDeliveryQueue"
        : `aura-worker-notification-delivery-${stage}`,
    );
    const percolatorMapping = mappings.find((mapping) =>
      JSON.stringify(mapping.Properties?.FunctionName).includes("SearchFilterPercolatorVersion"),
    );
    expect(percolatorMapping?.Properties).toMatchObject({
      BatchSize: 1,
      Enabled: percolatorActivation,
      FunctionResponseTypes: ["ReportBatchItemFailures"],
    });
    expect(JSON.stringify(percolatorMapping?.Properties?.EventSourceArn)).toContain(
      stage === "ephemeral"
        ? "WorkerQueuesSearchFilterPercolatorQueue"
        : `aura-worker-search-filter-percolator-${stage}`,
    );
    const notificationMappings = [
      {
        version: "SearchFilterMatchNotificationVersion",
        activation: matchNotificationActivation,
        queueLogicalId: "WorkerQueuesSearchFilterMatchNotificationQueue",
        queueName: "search-filter-match-notification",
      },
      {
        version: "WatchlistNotificationVersion",
        activation: watchlistNotificationActivation,
        queueLogicalId: "WorkerQueuesWatchlistNotificationQueue",
        queueName: "watchlist-notification",
      },
    ];
    for (const mappingContract of notificationMappings) {
      const mapping = mappings.find((candidate) =>
        JSON.stringify(candidate.Properties?.FunctionName).includes(mappingContract.version),
      );
      expect(mapping?.Properties).toMatchObject({
        BatchSize: 1,
        Enabled: mappingContract.activation,
        FunctionResponseTypes: ["ReportBatchItemFailures"],
      });
      expect(JSON.stringify(mapping?.Properties?.EventSourceArn)).toContain(
        stage === "ephemeral"
          ? mappingContract.queueLogicalId
          : `aura-worker-${mappingContract.queueName}-${stage}`,
      );
    }
    const cdcRouterMapping = mappings.find((mapping) => mapping.Properties?.BatchSize === 100);
    if (stage === "ephemeral") {
      expect(cdcRouterMapping).toBeUndefined();
    } else {
      expect(cdcRouterMapping?.Properties).toMatchObject({ Enabled: cdcRouterActivation });
    }

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
