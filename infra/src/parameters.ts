import * as cdk from "aws-cdk-lib";
import { Construct } from "constructs";

export interface ApplicationParameters {
  readonly commitSha: string;
  readonly productListingOpenSearchConsumerActivation: cdk.CfnCondition;
}

export function artifactCommitShaParameter(scope: Construct): string {
  return new cdk.CfnParameter(scope, "CommitSHA", {
    type: "String",
    description: "Artifact version to deploy. Reusing an older SHA rolls back Lambda/template artifacts.",
  }).valueAsString;
}

export function applicationParameters(scope: Construct): ApplicationParameters {
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

  return {
    commitSha,
    productListingOpenSearchConsumerActivation,
  };
}
