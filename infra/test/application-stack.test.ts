import * as cdk from "aws-cdk-lib";
import { Template } from "aws-cdk-lib/assertions";
import { ApplicationEphemeralStack, createApplicationStacks } from "../src/application-stack";
import { API_ROUTE_CATALOG } from "../src/constructs/api";
import { STAGES, type StageName } from "../src/config";

function createStacks(stage: StageName) {
  const app = new cdk.App({ analyticsReporting: false });
  return createApplicationStacks(app, {
    stage,
    stackNamePrefix: `application-${stage}`,
  });
}

describe("Application stacks", () => {
  test.each(STAGES)("synthesizes the %s stack contract", (stage) => {
    const stacks = createStacks(stage);


    expect(Object.values(Template.fromStack(stacks.compute).findResources("AWS::Cognito::UserPool"))).toHaveLength(1);
    expect(Object.values(Template.fromStack(stacks.api).findResources("AWS::ApiGatewayV2::Api"))).toHaveLength(1);
    expect(Object.values(Template.fromStack(stacks.api).findResources("AWS::ApiGatewayV2::Route"))).toHaveLength(API_ROUTE_CATALOG.length);
    expect(Object.values(Template.fromStack(stacks.compute).findResources("AWS::StepFunctions::StateMachine"))).toHaveLength(0);
  });

  test.each(STAGES)("does not synthesize native worker or Sequin ingress in %s", (stage) => {
    const stacks = createStacks(stage);
    for (const stack of [stacks.data, stacks.compute, stacks.api]) {
      const template = Template.fromStack(stack);
      expect(JSON.stringify(template.toJSON())).not.toMatch(/aura-historia-worker|\/cdc\/sequin|AURA_HISTORIA_WORKER_/i);
    }
  });

  test("synthesizes the ephemeral stack without CloudFront", () => {
    const app = new cdk.App({ analyticsReporting: false });
    const template = Template.fromStack(new ApplicationEphemeralStack(app, "application-ephemeral", { stage: "ephemeral" }));

    expect(Object.values(template.findResources("AWS::OpenSearchService::Domain"))).toHaveLength(1);
    expect(Object.values(template.findResources("AWS::CloudFront::Distribution"))).toHaveLength(0);
  });
});
