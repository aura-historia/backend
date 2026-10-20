import type { StageName } from "../config";
import { Construct } from "constructs";
import { createCloudFrontWebAcl } from "./cloudfront-web-acl";

/** Must equal the transport's whole-request limit, including undeclared/chunked bodies. */
export const MCP_MAX_REQUEST_BODY_BYTES = 256 * 1024;
/** Decimal Content-Length strictly greater than 262144, including leading zeroes. */
export const MCP_OVERSIZED_CONTENT_LENGTH_PATTERN =
  "^0*(?:[1-9][0-9]{6,}|[3-9][0-9]{5}|2[7-9][0-9]{4}|26[3-9][0-9]{3}|262[2-9][0-9]{2}|2621[5-9][0-9]|26214[5-9])$";

export function createMcpWebAcl(scope: Construct, stage: StageName): string {
  return createCloudFrontWebAcl(scope, {
    id: "McpWebAcl",
    providerFunctionName: `mcp-cloudfront-web-acl-provider-${stage}`,
    description: `Aura Historia ${stage} MCP CloudFront Web ACL`,
    metricName: `mcp-cloudfront-${stage}`,
    name: `application-${stage}-mcp-cloudfront-web-acl`,
    sampledRequestsEnabled: false,
    customResponseBodies: {
      "payload-too-large": {
        Content: JSON.stringify({ error: "request_too_large" }),
        ContentType: "APPLICATION_JSON",
      },
    },
    // Five rules fit the same CloudFront plan as REST, without sharing REST exceptions.
    rules: [
      requestBodyLimitRule(),
      managedRule("AWSManagedRulesAmazonIpReputationList", 1),
      managedRule("AWSManagedRulesCommonRuleSet", 2, ["NoUserAgent_HEADER", "SizeRestrictions_BODY"]),
      bodySizeOutsideProtocolRule(),
      managedRule("AWSManagedRulesKnownBadInputsRuleSet", 4),
    ],
  });
}

function visibility(name: string) {
  return { CloudWatchMetricsEnabled: true, MetricName: name, SampledRequestsEnabled: false };
}

function managedRule(name: string, priority: number, countedRules: string[] = []) {
  return {
    Name: `AWS-${name}`,
    OverrideAction: { None: {} },
    Priority: priority,
    Statement: {
      ManagedRuleGroupStatement: {
        Name: name,
        VendorName: "AWS",
        ...(countedRules.length === 0 ? {} : {
          RuleActionOverrides: countedRules.map((rule) => ({ Name: rule, ActionToUse: { Count: {} } })),
        }),
      },
    },
    VisibilityConfig: visibility(`AWS-${name}`),
  };
}

function requestBodyLimitRule() {
  const name = "MCP-RequestBodyLimit";
  return {
    Name: name,
    Priority: 0,
    Action: { Block: { CustomResponse: {
      CustomResponseBodyKey: "payload-too-large",
      ResponseCode: 413,
      ResponseHeaders: [{ Name: "Cache-Control", Value: "no-store" }],
    } } },
    Statement: {
      RegexMatchStatement: {
        FieldToMatch: { SingleHeader: { Name: "content-length" } },
        RegexString: MCP_OVERSIZED_CONTENT_LENGTH_PATTERN,
        TextTransformations: [{ Priority: 0, Type: "NONE" }],
      },
    },
    VisibilityConfig: visibility(name),
  };
}

/** Only POST /v1 may exceed the managed 8 KiB body threshold; other rules still inspect it. */
function bodySizeOutsideProtocolRule() {
  const name = "MCP-BodySizeOutsideProtocol";
  return {
    Name: name,
    Priority: 3,
    Action: { Block: { CustomResponse: {
      ResponseCode: 403,
      ResponseHeaders: [{ Name: "Cache-Control", Value: "no-store" }],
    } } },
    Statement: {
      AndStatement: {
        Statements: [
          { LabelMatchStatement: { Key: "awswaf:managed:aws:core-rule-set:SizeRestrictions_Body", Scope: "LABEL" } },
          { NotStatement: { Statement: {
            AndStatement: { Statements: [
              { RegexMatchStatement: {
                FieldToMatch: { UriPath: {} }, RegexString: "^/v1$",
                TextTransformations: [{ Priority: 0, Type: "NONE" }],
              } },
              { RegexMatchStatement: {
                FieldToMatch: { Method: {} }, RegexString: "^POST$",
                TextTransformations: [{ Priority: 0, Type: "NONE" }],
              } },
            ] },
          } } },
        ],
      },
    },
    VisibilityConfig: visibility(name),
  };
}
