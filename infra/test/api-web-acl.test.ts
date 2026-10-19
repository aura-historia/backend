import * as cdk from "aws-cdk-lib";
import { Template } from "aws-cdk-lib/assertions";
import * as fs from "node:fs";
import * as path from "node:path";
import { createApplicationStacks } from "../src/application-stack";
import { API_MAX_REQUEST_BODY_BYTES, API_OVERSIZED_CONTENT_LENGTH_PATTERN } from "../src/constructs/api";
import type { StageName } from "../src/config";

type WebAclRule = {
  readonly Action?: { readonly Block?: { readonly CustomResponse?: { CustomResponseBodyKey: string; ResponseCode: number } } };
  readonly Name: string;
  readonly OverrideAction?: unknown;
  readonly Priority: number;
  readonly Statement: {
    readonly ManagedRuleGroupStatement?: { Name: string; RuleActionOverrides?: { ActionToUse: unknown; Name: string }[] };
    readonly RegexMatchStatement?: { FieldToMatch: unknown; RegexString: string; TextTransformations: unknown[] };
  };
};

type WebAclProperties = {
  readonly CustomResponseBodies: Record<string, { Content: string; ContentType: string }>;
  readonly Rules: WebAclRule[];
};

function webAclProperties(stage: StageName): WebAclProperties {
  const app = new cdk.App({ analyticsReporting: false });
  const template = Template.fromStack(createApplicationStacks(app, { stage }).api);
  const webAcls = Object.entries(template.findResources("AWS::CloudFormation::CustomResource"))
    .filter(([id]) => id.startsWith("ApiWebAcl") || id.includes("ApiWebAcl"))
    .map(([, resource]) => (resource as { Properties: WebAclProperties }).Properties);
  expect(webAcls).toHaveLength(1);
  return webAcls[0];
}

function rule(properties: WebAclProperties, name: string): WebAclRule {
  const match = properties.Rules.find((candidate) => candidate.Name === name);
  if (!match) {
    throw new Error(`Missing Web ACL rule ${name}.`);
  }
  return match;
}

describe("API CloudFront Web ACL", () => {
  test.each(["dev", "prod"] as const)("%s keeps managed protections and counts only the reviewed managed rules", (stage) => {
    const properties = webAclProperties(stage);

    expect(properties.Rules.map((candidate) => candidate.Name)).toEqual([
      "AWS-AWSManagedRulesAmazonIpReputationList",
      "AWS-AWSManagedRulesCommonRuleSet",
      "AWS-AWSManagedRulesKnownBadInputsRuleSet",
      "API-RequestBodyLimit",
    ]);
    expect(properties.Rules.map((candidate) => candidate.Priority)).toEqual([0, 1, 2, 3]);
    // CloudFront flat-rate Free plan allows five WAF rules.
    expect(properties.Rules.length).toBeLessThanOrEqual(5);
    for (const managed of properties.Rules.filter((candidate) => candidate.Statement.ManagedRuleGroupStatement)) {
      expect(managed.OverrideAction).toEqual({ None: {} });
    }
    expect(rule(properties, "AWS-AWSManagedRulesCommonRuleSet").Statement.ManagedRuleGroupStatement?.RuleActionOverrides).toEqual([
      { ActionToUse: { Count: {} }, Name: "NoUserAgent_HEADER" },
      { ActionToUse: { Count: {} }, Name: "SizeRestrictions_BODY" },
    ]);
    expect(rule(properties, "AWS-AWSManagedRulesAmazonIpReputationList").Statement.ManagedRuleGroupStatement?.RuleActionOverrides).toBeUndefined();
    expect(rule(properties, "AWS-AWSManagedRulesKnownBadInputsRuleSet").Statement.ManagedRuleGroupStatement?.RuleActionOverrides).toBeUndefined();
  });

  test.each(["dev", "prod"] as const)("%s blocks declared bodies above the application limit with the API 413 error", (stage) => {
    const properties = webAclProperties(stage);
    const limit = rule(properties, "API-RequestBodyLimit");

    expect(limit.Statement.RegexMatchStatement).toEqual({
      FieldToMatch: { SingleHeader: { Name: "content-length" } },
      RegexString: API_OVERSIZED_CONTENT_LENGTH_PATTERN,
      TextTransformations: [{ Priority: 0, Type: "NONE" }],
    });
    const customResponse = limit.Action?.Block?.CustomResponse;
    expect(customResponse?.ResponseCode).toBe(413);
    const body = properties.CustomResponseBodies[customResponse?.CustomResponseBodyKey ?? ""];
    expect(body.ContentType).toBe("APPLICATION_JSON");
    expect(JSON.parse(body.Content)).toEqual({ status: 413, title: "Payload Too Large", error: "BAD_BODY_VALUE" });
  });

  test("edge body limit equals the Axum whole-request limit", () => {
    const transport = fs.readFileSync(path.join(__dirname, "../../src/aura-historia-api/src/transport.rs"), "utf8");
    const match = /const MAX_REQUEST_BODY_BYTES: usize = ([0-9_]+);/.exec(transport);
    expect(match).not.toBeNull();
    expect(Number(match?.[1].replaceAll("_", ""))).toBe(API_MAX_REQUEST_BODY_BYTES);
  });

  test("oversized Content-Length pattern matches exactly the values above the limit", () => {
    const pattern = new RegExp(API_OVERSIZED_CONTENT_LENGTH_PATTERN);
    const oversized = (value: number | string) => pattern.test(String(value));

    for (let value = 0; value <= 2_100_000; value += 1) {
      if (oversized(value) !== value > API_MAX_REQUEST_BODY_BYTES) {
        throw new Error(`Content-Length ${value} classified incorrectly.`);
      }
    }
    for (let digits = 8; digits <= 20; digits += 1) {
      expect(oversized(`1${"0".repeat(digits - 1)}`)).toBe(true);
      expect(oversized("9".repeat(digits))).toBe(true);
    }
    expect(oversized("9876")).toBe(false);
    expect(oversized("0001048576")).toBe(false);
    expect(oversized("0001048577")).toBe(true);
    expect(oversized("")).toBe(false);
    expect(oversized("1048577 ")).toBe(false);
  });
});
