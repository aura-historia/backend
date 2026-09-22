import * as cdk from "aws-cdk-lib";
import { Construct } from "constructs";

export interface ApplicationParameters {
  readonly commitSha: string;
  readonly productListingOpenSearchConsumerActivation: cdk.CfnCondition;
  readonly productListingNormalizationConsumerActivation: cdk.CfnCondition;
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
  const productListingOpenSearchConsumerEnabled = new cdk.CfnParameter(scope, "ProductListingOpenSearchConsumerEnabled", {
    type: "String",
    default: "false",
    allowedValues: ["true", "false"],
    description: "Enable ProductListing projection, partner event sources, and FX scheduling after initialization readiness checks.",
  });
  const productListingOpenSearchConsumerActivation = new cdk.CfnCondition(scope, "ProductListingOpenSearchConsumerActivation", {
    expression: cdk.Fn.conditionEquals(productListingOpenSearchConsumerEnabled.valueAsString, "true"),
  });
  const productListingNormalizationConsumerEnabled = new cdk.CfnParameter(scope, "ProductListingNormalizationConsumerEnabled", {
    type: "String",
    default: "false",
    allowedValues: ["true", "false"],
    description: "Enable the dedicated ProductListing raw-normalization SQS Lambda after native-consumer and scheduled-reconciliation cutover gates.",
  });
  const productListingNormalizationConsumerActivation = new cdk.CfnCondition(scope, "ProductListingNormalizationConsumerActivation", {
    expression: cdk.Fn.conditionEquals(productListingNormalizationConsumerEnabled.valueAsString, "true"),
  });
  const cdcRouterActivation = includeCdcRouterActivation
    ? cdcRouterCondition(scope)
    : undefined;

  return {
    commitSha,
    productListingOpenSearchConsumerActivation,
    productListingNormalizationConsumerActivation,
    cdcRouterActivation,
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
