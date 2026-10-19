jest.mock("@aws-sdk/client-wafv2", () => ({
  CreateWebACLCommand: jest.fn(),
  DeleteWebACLCommand: jest.fn(),
  GetWebACLCommand: jest.fn(),
  ListWebACLsCommand: jest.fn(),
  UpdateWebACLCommand: jest.fn(),
  WAFV2Client: jest.fn(),
}), { virtual: true });

const provider = require("../src/resources/api-web-acl-custom-resource.js");

describe("API Web ACL custom resource", () => {
  test("normalizes CloudFormation string values for WAF", () => {
    const props = provider._test.normalizedProperties({
      MetricName: "api-cloudfront-dev",
      Name: "application-dev-api-cloudfront-web-acl",
      Rules: [
        {
          Name: "AWS-AWSManagedRulesAmazonIpReputationList",
          OverrideAction: { None: {} },
          Priority: "0",
          Statement: {
            ManagedRuleGroupStatement: {
              Name: "AWSManagedRulesAmazonIpReputationList",
              VendorName: "AWS",
            },
          },
          VisibilityConfig: {
            CloudWatchMetricsEnabled: "true",
            MetricName: "AWS-AWSManagedRulesAmazonIpReputationList",
            SampledRequestsEnabled: "true",
          },
        },
      ],
      Scope: "CLOUDFRONT",
    });

    const input = provider._test.webAclInput(props);

    expect(input.Rules[0].Priority).toBe(0);
    expect(input.Rules[0].VisibilityConfig.CloudWatchMetricsEnabled).toBe(true);
    expect(input.Rules[0].VisibilityConfig.SampledRequestsEnabled).toBe(true);
  });

  test("passes custom response bodies and numeric response codes to WAF", () => {
    const customResponseBodies = {
      "payload-too-large": { Content: "{\"status\":413}", ContentType: "APPLICATION_JSON" },
    };
    const props = provider._test.normalizedProperties({
      CustomResponseBodies: customResponseBodies,
      MetricName: "api-cloudfront-dev",
      Name: "application-dev-api-cloudfront-web-acl",
      Rules: [
        {
          Action: { Block: { CustomResponse: { CustomResponseBodyKey: "payload-too-large", ResponseCode: "413" } } },
          Name: "API-RequestBodyLimit",
          Priority: "3",
          Statement: {
            RegexMatchStatement: {
              FieldToMatch: { SingleHeader: { Name: "content-length" } },
              RegexString: "^[0-9]{8,}$",
              TextTransformations: [{ Priority: "0", Type: "NONE" }],
            },
          },
          VisibilityConfig: {
            CloudWatchMetricsEnabled: "true",
            MetricName: "API-RequestBodyLimit",
            SampledRequestsEnabled: "true",
          },
        },
      ],
      Scope: "CLOUDFRONT",
    });

    const input = provider._test.webAclInput(props);

    expect(input.CustomResponseBodies).toEqual(customResponseBodies);
    expect(input.Rules[0].Action.Block.CustomResponse.ResponseCode).toBe(413);
    expect(input.Rules[0].Statement.RegexMatchStatement.TextTransformations[0].Priority).toBe(0);
  });

  test("omits custom response bodies when none are configured", () => {
    const props = provider._test.normalizedProperties({ Name: "acl", Rules: [] });

    expect(provider._test.webAclInput(props)).not.toHaveProperty("CustomResponseBodies");
  });
});
