import * as cdk from "aws-cdk-lib";
import { Template } from "aws-cdk-lib/assertions";
import { createApplicationStacks } from "../src/application-stack";
import { STAGES, type StageName } from "../src/config";
import { consentEvidenceLogGroupNames } from "../src/constructs/lambdas";

type CloudFormationResource = {
  readonly Type: string;
  readonly Properties: Record<string, any>;
};

function templatesFor(stage: StageName): Template[] {
  const app = new cdk.App({ analyticsReporting: false });
  const stacks = createApplicationStacks(app, { stage });
  return Object.values(stacks)
    .filter((stack): stack is cdk.Stack => stack !== undefined)
    .map((stack) => Template.fromStack(stack));
}

function resources(templates: Template[], type: string): [string, CloudFormationResource][] {
  return templates.flatMap((template) => Object.entries(template.findResources(type)) as [string, CloudFormationResource][]);
}

/** The stage's own Lambda functions: native binaries on the custom runtime, named by stage. */
function stageFunctions(templates: Template[], stage: StageName): CloudFormationResource[] {
  return resources(templates, "AWS::Lambda::Function")
    .map(([, resource]) => resource)
    .filter((resource) => resource.Properties.Runtime === "provided.al2023"
      && String(resource.Properties.FunctionName).endsWith(`-${stage}`));
}

describe.each(STAGES)("%s Lambda logging", (stage) => {
  let templates: Template[];
  beforeAll(() => { templates = templatesFor(stage); });

  test("every function logs to a log group its stack owns, in JSON, with the system log level WARN", () => {
    let checked = 0;
    for (const template of templates) {
      // A log group is referenced by its logical ID, which is local to the stack.
      const groups = new Map(resources([template], "AWS::Logs::LogGroup")
        .map(([logicalId, group]) => [logicalId, group.Properties.LogGroupName as string]));
      for (const fn of stageFunctions([template], stage)) {
        const config = fn.Properties.LoggingConfig;
        expect(config?.LogFormat).toBe("JSON");
        expect(config?.SystemLogLevel).toBe("WARN");
        expect(groups.get(config?.LogGroup?.Ref)).toBe(`/aws/lambda/${fn.Properties.FunctionName}`);
        checked += 1;
      }
    }
    expect(checked).toBeGreaterThan(25);
  });

  // Lambda assigns INFO to every JSON line without an RFC 3339 timestamp and to plain-text panic
  // output, which the application emits, so an application filter of WARN would drop its errors.
  test("never lets Lambda filter application logs at WARN or above; the application filters itself", () => {
    for (const fn of stageFunctions(templates, stage)) {
      expect(["TRACE", "DEBUG"]).toContain(fn.Properties.LoggingConfig.ApplicationLogLevel);
    }
  });

  test("sets the application level through LOG_LEVEL, warn in production", () => {
    for (const fn of stageFunctions(templates, stage)) {
      expect(fn.Properties.Environment.Variables.LOG_LEVEL).toBe(stage === "prod" ? "warn" : "info");
    }
  });

  test("expires every log group except the consent evidence groups", () => {
    const evidence = new Set<string>(consentEvidenceLogGroupNames(stage));
    const groups = resources(templates, "AWS::Logs::LogGroup").map(([, group]) => group);
    expect(groups.length).toBeGreaterThan(20);
    for (const group of groups) {
      const name = String(group.Properties.LogGroupName);
      if (evidence.has(name)) {
        expect(group.Properties.RetentionInDays).toBeUndefined();
      } else {
        expect([7, 30, 90]).toContain(group.Properties.RetentionInDays);
      }
    }
  });

  test("keeps every Lambda log group for a week unless it holds another retention on purpose", () => {
    const evidence = new Set<string>(consentEvidenceLogGroupNames(stage));
    const lambdaGroups = resources(templates, "AWS::Logs::LogGroup")
      .map(([, group]) => group)
      .filter((group) => String(group.Properties.LogGroupName).startsWith("/aws/lambda/")
        && !evidence.has(group.Properties.LogGroupName));
    const longer = lambdaGroups
      .filter((group) => group.Properties.RetentionInDays !== 7)
      .map((group) => group.Properties.LogGroupName)
      .sort();
    expect(longer).toEqual([
      `/aws/lambda/aura-historia-mcp-${stage}`,
      `/aws/lambda/product-listing-ingestion-lambda-${stage}`,
    ]);
  });

  test("writes no API Gateway access log", () => {
    for (const template of templates) {
      for (const stageResource of Object.values(template.findResources("AWS::ApiGatewayV2::Stage")) as CloudFormationResource[]) {
        expect(stageResource.Properties.AccessLogSettings).toBeUndefined();
      }
    }
    expect(resources(templates, "AWS::Logs::LogGroup")
      .filter(([, group]) => String(group.Properties.LogGroupName).startsWith("/aws/apigateway/"))).toEqual([]);
  });
});
