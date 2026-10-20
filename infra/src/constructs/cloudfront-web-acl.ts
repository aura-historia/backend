import * as cdk from "aws-cdk-lib";
import * as iam from "aws-cdk-lib/aws-iam";
import * as lambda from "aws-cdk-lib/aws-lambda";
import * as fs from "node:fs";
import * as path from "node:path";
import { Construct } from "constructs";

interface CloudFrontWebAclProps {
  readonly id: string;
  readonly providerFunctionName: string;
  readonly description: string;
  readonly metricName: string;
  readonly name: string;
  readonly customResponseBodies: Record<string, unknown>;
  readonly rules: unknown[];
  readonly sampledRequestsEnabled?: boolean;
}

/** The workload stack is regional; CloudFront WAF resources must live in us-east-1. */
export function createCloudFrontWebAcl(scope: Construct, props: CloudFrontWebAclProps): string {
  const provider = new lambda.Function(scope, `${props.id}CustomResourceFunction`, {
    functionName: props.providerFunctionName,
    runtime: lambda.Runtime.NODEJS_24_X,
    handler: "index.handler",
    timeout: cdk.Duration.minutes(2),
    code: lambda.Code.fromInline(fs.readFileSync(
      path.join(__dirname, "..", "resources", "api-web-acl-custom-resource.js"), "utf8",
    )),
  });

  provider.addToRolePolicy(new iam.PolicyStatement({
    actions: [
      "wafv2:CreateWebACL",
      "wafv2:DeleteWebACL",
      "wafv2:GetWebACL",
      "wafv2:ListWebACLs",
      "wafv2:TagResource",
      "wafv2:UpdateWebACL",
    ],
    resources: ["*"],
  }));

  const webAcl = new cdk.CustomResource(scope, props.id, {
    serviceToken: provider.functionArn,
    properties: {
      Description: props.description,
      MetricName: props.metricName,
      Name: props.name,
      CustomResponseBodies: props.customResponseBodies,
      Region: "us-east-1",
      Rules: props.rules,
      Scope: "CLOUDFRONT",
      ...(props.sampledRequestsEnabled === undefined ? {} : { SampledRequestsEnabled: props.sampledRequestsEnabled }),
    },
  });
  return webAcl.getAttString("WebAclArn");
}
