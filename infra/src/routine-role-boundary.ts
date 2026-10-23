import * as cdk from "aws-cdk-lib";
import * as iam from "aws-cdk-lib/aws-iam";
import type { IConstruct } from "constructs";
import type { StageName } from "./config";

export function boundaryArn(stack: cdk.Stack, stage: StageName, kind: "runtime" | "migration"): string {
  return stack.formatArn({ service: "iam", region: "", resource: "policy",
    resourceName: `aura-historia/delivery/${kind}-${stage}`, arnFormat: cdk.ArnFormat.SLASH_RESOURCE_NAME });
}

/** Routine stacks can create only roles constrained by the separately protected delivery boundary. */
export function constrainRoutineRoles(stack: cdk.Stack, stage: StageName, kind: "runtime" | "migration"): void {
  cdk.Aspects.of(stack).add({ visit(node: IConstruct) {
    if (node instanceof iam.CfnRole) {
      node.path = `/aura-historia/${kind}/${stage}/`;
      node.permissionsBoundary = boundaryArn(stack, stage, kind);
    }
  } });
}

