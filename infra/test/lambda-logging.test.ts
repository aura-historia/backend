import * as cdk from "aws-cdk-lib";
import { Template } from "aws-cdk-lib/assertions";
import { createApplicationStacks } from "../src/application-stack";
import { STAGES, type StageName } from "../src/config";

type CloudFormationResource = {
  readonly Type: string;
  readonly Properties: Record<string, any>;
};

function identityTemplate(stage: StageName): Template {
  return Template.fromStack(createApplicationStacks(new cdk.App({ analyticsReporting: false }), { stage }).identity);
}

function resources(template: Template, type: string): [string, CloudFormationResource][] {
  return Object.entries(template.findResources(type)) as [string, CloudFormationResource][];
}

describe.each(STAGES)("%s Cognito trigger logging", (stage) => {
  let template: Template;
  let triggers: CloudFormationResource[];
  beforeAll(() => {
    template = identityTemplate(stage);
    triggers = resources(template, "AWS::Lambda::Function").map(([, resource]) => resource);
  });

  test("every trigger logs to a log group its stack owns, in JSON, with the system log level WARN", () => {
    const groups = new Map(resources(template, "AWS::Logs::LogGroup")
      .map(([logicalId, group]) => [logicalId, group.Properties.LogGroupName as string]));
    expect(triggers).toHaveLength(2);
    for (const fn of triggers) {
      const config = fn.Properties.LoggingConfig;
      expect(config?.LogFormat).toBe("JSON");
      expect(config?.SystemLogLevel).toBe("WARN");
      expect(groups.get(config?.LogGroup?.Ref)).toBe(`/aws/lambda/${fn.Properties.FunctionName}`);
    }
  });

  // Lambda assigns INFO to every JSON line without an RFC 3339 timestamp and to plain-text panic
  // output, which the application emits, so an application filter of WARN would drop its errors.
  test("never lets Lambda filter application logs at WARN or above; the application filters itself through LOG_LEVEL", () => {
    for (const fn of triggers) {
      expect(["TRACE", "DEBUG"]).toContain(fn.Properties.LoggingConfig.ApplicationLogLevel);
      expect(fn.Properties.Environment.Variables.LOG_LEVEL).toBe("warn");
    }
  });

  test("keeps trigger logs for a week", () => {
    const groups = resources(template, "AWS::Logs::LogGroup").map(([, group]) => group);
    expect(groups).toHaveLength(2);
    for (const group of groups) expect(group.Properties.RetentionInDays).toBe(7);
  });
});
