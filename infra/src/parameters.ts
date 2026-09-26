import * as cdk from "aws-cdk-lib";
import { Construct } from "constructs";

export interface ApplicationParameters {
  readonly commitSha: string;
  readonly cdcRouterActivation?: cdk.CfnCondition;
}

export function artifactCommitShaParameter(scope: Construct): string {
  return new cdk.CfnParameter(scope, "CommitSHA", {
    type: "String",
    description: "Artifact version to deploy. Reusing an older SHA rolls back Lambda/template artifacts.",
  }).valueAsString;
}

export function applicationParameters(scope: Construct, includeCdcRouterActivation = false): ApplicationParameters {
  const commitSha = artifactCommitShaParameter(scope);
  const cdcRouterActivation = includeCdcRouterActivation
    ? cdcRouterCondition(scope)
    : undefined;

  return { commitSha, cdcRouterActivation };
}

function cdcRouterCondition(scope: Construct): cdk.CfnCondition {
  const cdcRouterEnabled = new cdk.CfnParameter(scope, "CdcRouterEnabled", {
    type: "String",
    default: "false",
    allowedValues: ["true", "false"],
    description: "Enable the DMS/Kinesis CDC router only after separately approved slot, task-start, and delivery evidence gates.",
  });
  return new cdk.CfnCondition(scope, "CdcRouterActivation", {
    expression: cdk.Fn.conditionEquals(cdcRouterEnabled.valueAsString, "true"),
  });
}
