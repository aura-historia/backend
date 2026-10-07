import * as cdk from "aws-cdk-lib";
import { Construct } from "constructs";

export interface ApplicationParameters {
  readonly commitSha: string;
  readonly cdcRouterActivation: cdk.CfnCondition;
  readonly searchFilterClassifierModel: string;
  readonly searchFilterMatchShouldShowThresholdBps: string;
}

export function artifactCommitShaParameter(scope: Construct): string {
  return new cdk.CfnParameter(scope, "CommitSHA", {
    type: "String",
    description: "Artifact version to deploy. Reusing an older SHA rolls back Lambda/template artifacts.",
  }).valueAsString;
}

export function applicationParameters(
  scope: Construct,
  searchFilterClassifierDefaults: {
    readonly model: "clef-flash" | "clef";
    readonly shouldShowThresholdBps: number;
  } = { model: "clef-flash", shouldShowThresholdBps: 5_000 },
): ApplicationParameters {
  const commitSha = artifactCommitShaParameter(scope);
  const searchFilterClassifierModel = new cdk.CfnParameter(scope, "SearchFilterClassifierModel", {
    type: "String",
    default: searchFilterClassifierDefaults.model,
    allowedValues: ["clef-flash", "clef"],
    description: "Cloudflare Clef model used for enhanced saved-search matching.",
  }).valueAsString;
  const searchFilterMatchShouldShowThresholdBps = new cdk.CfnParameter(
    scope,
    "SearchFilterMatchShouldShowThresholdBps",
    {
      type: "Number",
      default: searchFilterClassifierDefaults.shouldShowThresholdBps,
      minValue: 0,
      maxValue: 10_000,
      description: "Inclusive should_show acceptance threshold in basis points for enhanced saved-search matching.",
    },
  ).valueAsString;
  const cdcRouterActivation = cdcRouterCondition(scope);

  return {
    commitSha,
    cdcRouterActivation,
    searchFilterClassifierModel,
    searchFilterMatchShouldShowThresholdBps,
  };
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
