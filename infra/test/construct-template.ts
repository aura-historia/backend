import * as cdk from "aws-cdk-lib";
import { Template } from "aws-cdk-lib/assertions";
import type { Construct } from "constructs";

/** Assert each front door's complete resource inventory without mixing the two API namespaces. */
export function templateForConstruct(stack: cdk.Stack, construct: Construct): Template {
  const template = Template.fromStack(stack).toJSON();
  const resourceIds = new Set(construct.node.findAll()
    .filter(cdk.CfnResource.isCfnResource)
    .map((resource) => stack.resolve(resource.logicalId) as string));
  return Template.fromJSON({
    ...template,
    Resources: Object.fromEntries(Object.entries(template.Resources)
      .filter(([id]) => resourceIds.has(id))),
  });
}
