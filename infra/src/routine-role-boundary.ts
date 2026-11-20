import * as cdk from "aws-cdk-lib";
import * as iam from "aws-cdk-lib/aws-iam";
import type { IConstruct } from "constructs";
import type { StageName } from "./config";

/** The retained delivery permissions boundary that the deployed API stack roles carry. */
export function boundaryArn(stack: cdk.Stack, stage: StageName): string {
  return stack.formatArn({ service: "iam", region: "", resource: "policy",
    resourceName: `aura-historia/delivery/runtime-${stage}`, arnFormat: cdk.ArnFormat.SLASH_RESOURCE_NAME });
}

/**
 * Keeps the path and boundary the API stack's roles were deployed with. A role's path cannot
 * change without replacing the role, and the boundary policy is retained when its stack is deleted.
 */
export function constrainRoutineRoles(stack: cdk.Stack, stage: StageName): void {
  cdk.Aspects.of(stack).add({ visit(node: IConstruct) {
    if (node instanceof iam.CfnRole) {
      node.path = `/aura-historia/runtime/${stage}/`;
      node.permissionsBoundary = boundaryArn(stack, stage);
    }
  } });
}
