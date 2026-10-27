import * as cdk from "aws-cdk-lib";
import { Template } from "aws-cdk-lib/assertions";
import { createApplicationStacks } from "../src/application-stack";
import { STAGES, type StageName } from "../src/config";

type CloudFormationResource = {
  readonly Type: string;
  readonly Properties: Record<string, any>;
};

// Independent expected policy: synth must preserve dev and reserve modest production capacity.
const PROD_RESERVATIONS: Record<string, number> = {
  "aura-historia-api": 50,
  "aura-historia-mcp": 5,
  "cloudwatch-log-retention-lambda": 1,
  "cdc-router-lambda": 2,
  "backend-cleanup-lambda": 1,
  "cognito-post-confirmation": 5,
  "cognito-pre-sign-up": 5,
  "shopify-lambda": 5,
  "stripe-lambda": 5,
  "product-listing-opensearch-lambda": 10,
  "product-listing-ingestion-lambda": 2,
  "product-listing-normalization-lambda": 5,
  "product-content-assessment-lambda": 5,
  "product-embedding-lambda": 5,
  "product-translation-lambda": 5,
  "notification-delivery-lambda": 5,
  "marketing-consent-sync-lambda": 2,
  "user-identity-deletion-lambda": 2,
  "listing-geography-lambda": 2,
  "listing-shipping-lambda": 2,
  "geo-resolution-lambda": 2,
  "search-filter-match-notification-lambda": 5,
  "watchlist-notification-lambda": 5,
};
const DEV_RESERVATIONS: Record<string, number> = {
  "aura-historia-mcp": 2,
  "product-listing-ingestion-lambda": 2,
  "listing-geography-lambda": 2,
  "listing-shipping-lambda": 2,
  "geo-resolution-lambda": 2,
};

function templatesFor(stage: StageName): Template[] {
  const app = new cdk.App({ analyticsReporting: false });
  const stacks = createApplicationStacks(app, { stage });
  return Object.values(stacks)
    .filter((stack): stack is cdk.Stack => stack !== undefined)
    .map((stack) => Template.fromStack(stack));
}

function resources(template: Template, type: string): CloudFormationResource[] {
  return Object.values(template.findResources(type)) as CloudFormationResource[];
}

function reservations(templates: Template[]): Record<string, number> {
  return Object.fromEntries(templates.flatMap((template) => resources(template, "AWS::Lambda::Function"))
    .filter((resource) => resource.Properties.ReservedConcurrentExecutions !== undefined)
    .map((resource) => [resource.Properties.FunctionName, resource.Properties.ReservedConcurrentExecutions]));
}

function mappingFunction(template: Template, mapping: CloudFormationResource): CloudFormationResource {
  const all = template.toJSON().Resources as Record<string, CloudFormationResource>;
  let target = all[mapping.Properties.FunctionName.Ref];
  if (target.Type === "AWS::Lambda::Version") {
    target = all[target.Properties.FunctionName.Ref];
  }
  expect(target.Type).toBe("AWS::Lambda::Function");
  return target;
}

describe.each(STAGES)("%s Lambda concurrency guard", (stage) => {
  let templates: Template[];
  beforeAll(() => { templates = templatesFor(stage); });

  test("reserves only the approved per-function capacity", () => {
    const expected = stage === "prod" ? PROD_RESERVATIONS : DEV_RESERVATIONS;
    expect(reservations(templates)).toEqual(Object.fromEntries(Object.entries(expected)
      .map(([binary, limit]) => [`${binary}-${stage}`, limit])));
  });

  test("bounds SQS polling by its function reservation across all mappings", () => {
    let sqsMappings = 0;
    for (const template of templates) {
      const totals = new Map<string, number>();
      for (const mapping of resources(template, "AWS::Lambda::EventSourceMapping")) {
        expect(mapping.Properties.ProvisionedPollerConfig).toBeUndefined();
        if (JSON.stringify(mapping.Properties.EventSourceArn).includes(":sqs:")) {
          sqsMappings++;
          const fn = mappingFunction(template, mapping);
          const reserved = fn.Properties.ReservedConcurrentExecutions;
          expect(mapping.Properties.ScalingConfig).toEqual(
            reserved === undefined ? undefined : { MaximumConcurrency: reserved },
          );
          if (reserved !== undefined) {
            expect(reserved).toBeGreaterThanOrEqual(2);
            const name = fn.Properties.FunctionName;
            const total = (totals.get(name) ?? 0) + mapping.Properties.ScalingConfig.MaximumConcurrency;
            totals.set(name, total);
            expect(total).toBeLessThanOrEqual(reserved);
          }
        } else {
          expect(mapping.Properties.ScalingConfig).toBeUndefined();
        }
      }
    }
    expect(sqsMappings).toBe(15);
  });

  test("does not provision execution environments", () => {
    for (const template of templates) {
      for (const type of ["AWS::Lambda::Function", "AWS::Lambda::Alias", "AWS::Lambda::Version"]) {
        for (const resource of resources(template, type)) {
          expect(resource.Properties.ProvisionedConcurrencyConfig).toBeUndefined();
        }
      }
    }
  });
});

test("leaves regional headroom when prod and dev share the 1,000-execution quota", () => {
  const total = STAGES.flatMap((stage) => Object.values(reservations(templatesFor(stage))))
    .reduce((sum, limit) => sum + limit, 0);
  expect(total).toBe(146);
  expect(1_000 - total).toBeGreaterThanOrEqual(100);
});
