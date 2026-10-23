import * as cdk from "aws-cdk-lib";
import { Template } from "aws-cdk-lib/assertions";
import { createApplicationStacks } from "../src/application-stack";
import { API_ROUTE_CATALOG } from "../src/constructs/api";
import { MCP_ROUTE_CATALOG } from "../src/constructs/mcp";
import { STAGES, isStageName, stageConfig, type StageName } from "../src/config";

function createStacks(stage: StageName) {
  const app = new cdk.App({ analyticsReporting: false });
  return createApplicationStacks(app, {
    stage,
    stackNamePrefix: `application-${stage}`,
  });
}

describe("Application stacks", () => {
  test("rejects the removed ephemeral CDK stage", () => {
    expect(isStageName("ephemeral")).toBe(false);
    expect(() => stageConfig("ephemeral" as StageName)).toThrow("Unsupported CDK stage 'ephemeral'");
    expect(STAGES).toEqual(["prod", "dev"]);
  });

  test.each(STAGES)("synthesizes the %s stack contract", (stage) => {
    const stacks = createStacks(stage);

    expect(Object.values(Template.fromStack(stacks.identity).findResources("AWS::Cognito::UserPool"))).toHaveLength(1);
    expect(Object.values(Template.fromStack(stacks.api).findResources("AWS::ApiGatewayV2::Api"))).toHaveLength(2);
    expect(Object.values(Template.fromStack(stacks.api).findResources("AWS::ApiGatewayV2::Route"))).toHaveLength(API_ROUTE_CATALOG.length + MCP_ROUTE_CATALOG.length);
    expect(Object.values(Template.fromStack(stacks.compute).findResources("AWS::StepFunctions::StateMachine"))).toHaveLength(0);
    expect(stacks.compute.dependencies).toContain(stacks.migration);
    expect(stacks.initialization.dependencies).not.toContain(stacks.compute);
  });

  test.each(STAGES)("does not synthesize removed saved-filter matching or projection artifacts in %s", (stage) => {
    const stacks = createStacks(stage);
    for (const stack of Object.values(stacks)) {
      if (stack) {
        expect(JSON.stringify(Template.fromStack(stack).toJSON())).not.toMatch(
          /SearchFilter(?:Projection|Percolator)|SEARCH_FILTER_(?:PROJECTION|PERCOLATOR)|search-filter-(?:projection|percolator)|filter-projector|\/percolator\/|user_search_filters/,
        );
      }
    }
  });

  test.each(STAGES)("does not synthesize native worker or Sequin ingress in %s", (stage) => {
    const stacks = createStacks(stage);
    for (const stack of [stacks.data, stacks.compute, stacks.api]) {
      const template = Template.fromStack(stack);
      expect(JSON.stringify(template.toJSON())).not.toMatch(/aura-historia-worker|\/cdc\/sequin|AURA_HISTORIA_WORKER_/i);
    }
  });
});
