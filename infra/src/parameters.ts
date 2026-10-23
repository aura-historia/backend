import * as cdk from "aws-cdk-lib";
import { Construct } from "constructs";

export interface ApplicationParameters {
  readonly commitSha: string;
  readonly mailTemplatePrefix: string;
  readonly cdcRouterActivation: cdk.CfnCondition;
  readonly searchFilterClassifierModel: string;
  readonly searchFilterMatchBatchCandidates: string;
  readonly searchFilterMatchShouldShowThresholdBps: string;
}

export function artifactCommitShaParameter(scope: Construct): string {
  return new cdk.CfnParameter(scope, "CommitSHA", {
    type: "String",
    description: "Source provenance for runtime diagnostics and email template lookup; code objects are pinned separately.",
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
  const mailTemplatePrefix = new cdk.CfnParameter(scope, "MailTemplatePrefix", {
    type: "String", allowedPattern: "^mail/[a-zA-Z0-9._-]+/$",
    description: "Immutable shared mail artifact prefix from the completed bundle.",
  }).valueAsString;
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
  const searchFilterMatchBatchCandidates = new cdk.CfnParameter(scope, "SearchFilterMatchBatchCandidates", {
    type: "Number",
    default: 1,
    allowedValues: Array.from({ length: 50 }, (_, index) => String(index + 1)),
    description: "Maximum text candidates per shared classifier request (1–50); provider budgets may require smaller groups. One is singleton mode; enable larger values only after the labelled quality/cost gate.",
  }).valueAsString;
  const cdcRouterActivation = cdcRouterCondition(scope);

  return {
    commitSha,
    mailTemplatePrefix,
    cdcRouterActivation,
    searchFilterClassifierModel,
    searchFilterMatchBatchCandidates,
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
