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
});
