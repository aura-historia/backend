import * as cdk from "aws-cdk-lib";
import { Template } from "aws-cdk-lib/assertions";
import { createApplicationStacks } from "../src/application-stack";
import { STAGES, stageConfig, type StageName } from "../src/config";

type CloudFormationResource = {
  readonly Properties?: Record<string, unknown>;
};

function templatesFor(stage: StageName): Template[] {

  const app = new cdk.App({ analyticsReporting: false });
  const stacks = createApplicationStacks(app, { stage });
  const stackList: cdk.Stack[] = [];
  for (const stack of [stacks.network, stacks.data, stacks.migration, stacks.compute, stacks.api, stacks.observability]) {
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
        expect(resource.Properties?.ReservedConcurrentExecutions).toBe(
          resource.Properties?.FunctionName === `aura-historia-mcp-${stage}` ? stageConfig(stage).mcp.reservedConcurrency
            : resource.Properties?.FunctionName === `product-listing-ingestion-lambda-${stage}` ? 2
            : resource.Properties?.FunctionName === `geo-resolution-lambda-${stage}` ? 2
            : resource.Properties?.FunctionName === `listing-geography-lambda-${stage}` || resource.Properties?.FunctionName === `listing-shipping-lambda-${stage}` ? 2 : undefined,
        );
      }

      for (const resource of resources(template, "AWS::Lambda::Alias")) {
        expect(resource.Properties?.ProvisionedConcurrencyConfig).toBeUndefined();
      }

      for (const resource of resources(template, "AWS::Lambda::EventSourceMapping")) {
        expect(resource.Properties?.ProvisionedPollerConfig).toBeUndefined();
        if (JSON.stringify(resource.Properties?.FunctionName).includes("ProductListingIngestionVersion") || JSON.stringify(resource.Properties?.FunctionName).includes("GeoResolutionVersion") || JSON.stringify(resource.Properties?.FunctionName).includes("ListingGeographyVersion") || JSON.stringify(resource.Properties?.FunctionName).includes("ListingShippingVersion")) {
          expect(resource.Properties?.ScalingConfig).toEqual({ MaximumConcurrency: 2 });
        } else {
          expect(resource.Properties?.ScalingConfig).toBeUndefined();
        }
      }
    }
  });
});
