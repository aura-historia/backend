import * as cdk from "aws-cdk-lib";
import { Template } from "aws-cdk-lib/assertions";
import { createApplicationStacks, type ApplicationStageStacks } from "../src/application-stack";
import { isStageName, STAGES, stageConfig, type StageName } from "../src/config";

function createStacks(): ApplicationStageStacks {
  return createApplicationStacks(new cdk.App({ analyticsReporting: false }), { stage: "prod" });
}

/** Resource types of the retired serverless runtime; none may come back into the app. */
const RETIRED_RESOURCE_TYPE = new RegExp([
  "AWS::EC2::", "AWS::RDS::", "AWS::DMS::", "AWS::Kinesis::", "AWS::SQS::",
  "AWS::ApiGatewayV2::", "AWS::ECS::", "AWS::ECR::", "AWS::Scheduler::", "AWS::Events::",
  "AWS::CodePipeline::", "AWS::CodeBuild::", "AWS::CodeDeploy::",
  "AWS::Lambda::EventSourceMapping", "AWS::StepFunctions::",
].map((prefix) => `^${prefix.replaceAll(":", "\\:")}`).join("|"));

describe("Application stacks", () => {
  test("declares production only; the host has no staging copy", () => {
    expect(STAGES).toEqual(["prod"]);
    expect(isStageName("dev")).toBe(false);
    expect(() => stageConfig("dev" as StageName)).toThrow("Unsupported CDK stage 'dev'");
  });

  test("keeps the retained stack names and adds the host and monitoring stacks, all termination-protected", () => {
    const stacks = createStacks();
    expect(Object.values(stacks).map((stack) => stack.stackName)).toEqual([
      "application-prod-host", "application-prod-identity", "application-prod-api",
      "application-prod-monitoring", "application-prod-edge-monitoring",
    ]);
    for (const stack of Object.values(stacks)) expect(stack.terminationProtection).toBe(true);
  });

  test("deploys the host stack first: the other stacks name its user and secret", () => {
    const stacks = createStacks();
    expect(stacks.host.dependencies).toEqual([]);
    expect(stacks.identity.dependencies).toEqual([stacks.host]);
    expect(stacks.api.dependencies).toEqual([stacks.host]);
    expect(stacks.monitoring.dependencies).toEqual([]);
    expect(stacks.edgeMonitoring.dependencies).toEqual([]);
  });

  test("puts only the CloudFront alarms in the edge metrics region, deployable without a bootstrap", () => {
    const stacks = createStacks();
    expect(stacks.edgeMonitoring.region).toBe("us-east-1");
    expect(stacks.edgeMonitoring.synthesizer).toBeInstanceOf(cdk.LegacyStackSynthesizer);
    for (const stack of [stacks.host, stacks.identity, stacks.api, stacks.monitoring]) {
      expect(cdk.Token.isUnresolved(stack.region)).toBe(true);
    }
  });

  test("declares no resource of the retired serverless runtime", () => {
    for (const stack of Object.values(createStacks())) {
      const types = Object.values(Template.fromStack(stack).toJSON().Resources as Record<string, { Type: string }>)
        .map((resource) => resource.Type);
      expect(types.filter((type) => RETIRED_RESOURCE_TYPE.test(type))).toEqual([]);
    }
  });

  test("runs only the Cognito triggers and the Web ACL providers as Lambda functions", () => {
    const names = Object.values(createStacks()).flatMap((stack) =>
      Object.values(Template.fromStack(stack).findResources("AWS::Lambda::Function"))
        .map((resource) => resource.Properties.FunctionName as string));
    expect(names.sort()).toEqual([
      "api-cloudfront-web-acl-provider-prod",
      "cognito-post-confirmation-forwarder-prod",
      "cognito-pre-sign-up-trigger-prod",
      "mcp-cloudfront-web-acl-provider-prod",
    ]);
  });

  test("puts no function in a VPC", () => {
    for (const stack of Object.values(createStacks())) {
      for (const resource of Object.values(Template.fromStack(stack).findResources("AWS::Lambda::Function"))) {
        expect(resource.Properties.VpcConfig).toBeUndefined();
      }
    }
  });
});
