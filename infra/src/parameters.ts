import * as cdk from "aws-cdk-lib";
import { Construct } from "constructs";

export interface ApplicationParameters {
  readonly commitSha: string;
  readonly productListingOpenSearchConsumerActivation: cdk.CfnCondition;
  readonly productListingNormalizationConsumerActivation: cdk.CfnCondition;
  readonly productContentAssessmentConsumerActivation: cdk.CfnCondition;
  readonly productEmbeddingConsumerActivation: cdk.CfnCondition;
  readonly searchFilterProjectionConsumerActivation: cdk.CfnCondition;
  readonly searchFilterPercolatorConsumerActivation: cdk.CfnCondition;
  readonly searchFilterMatchNotificationConsumerActivation: cdk.CfnCondition;
  readonly watchlistNotificationConsumerActivation: cdk.CfnCondition;
  readonly notificationDeliveryConsumerActivation: cdk.CfnCondition;
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
  const productContentAssessmentConsumerEnabled = new cdk.CfnParameter(scope, "ProductContentAssessmentConsumerEnabled", {
    type: "String",
    default: "false",
    allowedValues: ["true", "false"],
    description: "Enable the dedicated ProductListing content-assessment SQS Lambda after the native-consumer handoff is complete.",
  });
  const productContentAssessmentConsumerActivation = new cdk.CfnCondition(scope, "ProductContentAssessmentConsumerActivation", {
    expression: cdk.Fn.conditionEquals(productContentAssessmentConsumerEnabled.valueAsString, "true"),
  });
  const productEmbeddingConsumerEnabled = new cdk.CfnParameter(scope, "ProductEmbeddingConsumerEnabled", {
    type: "String",
    default: "false",
    allowedValues: ["true", "false"],
    description: "Enable the dedicated ProductListing embedding SQS Lambda only after the native-consumer handoff, provider quota, and capacity gates are approved.",
  });
  const productEmbeddingConsumerActivation = new cdk.CfnCondition(scope, "ProductEmbeddingConsumerActivation", {
    expression: cdk.Fn.conditionEquals(productEmbeddingConsumerEnabled.valueAsString, "true"),
  });
  const searchFilterProjectionConsumerEnabled = new cdk.CfnParameter(scope, "SearchFilterProjectionConsumerEnabled", {
    type: "String",
    default: "false",
    allowedValues: ["true", "false"],
    description: "Enable the dedicated saved-filter projection SQS Lambda after the native-consumer handoff and deletion-fence gates.",
  });
  const searchFilterProjectionConsumerActivation = new cdk.CfnCondition(scope, "SearchFilterProjectionConsumerActivation", {
    expression: cdk.Fn.conditionEquals(searchFilterProjectionConsumerEnabled.valueAsString, "true"),
  });
  const searchFilterPercolatorConsumerEnabled = new cdk.CfnParameter(scope, "SearchFilterPercolatorConsumerEnabled", {
    type: "String",
    default: "false",
    allowedValues: ["true", "false"],
    description: "Enable the dedicated saved-filter percolator SQS Lambda after the native-consumer cutover gate.",
  });
  const searchFilterPercolatorConsumerActivation = new cdk.CfnCondition(scope, "SearchFilterPercolatorConsumerActivation", {
    expression: cdk.Fn.conditionEquals(searchFilterPercolatorConsumerEnabled.valueAsString, "true"),
  });
  const searchFilterMatchNotificationConsumerEnabled = new cdk.CfnParameter(scope, "SearchFilterMatchNotificationConsumerEnabled", {
    type: "String",
    default: "false",
    allowedValues: ["true", "false"],
    description: "Enable the dedicated saved-filter match notification SQS Lambda after the native-consumer handoff is complete.",
  });
  const searchFilterMatchNotificationConsumerActivation = new cdk.CfnCondition(scope, "SearchFilterMatchNotificationConsumerActivation", {
    expression: cdk.Fn.conditionEquals(searchFilterMatchNotificationConsumerEnabled.valueAsString, "true"),
  });
  const watchlistNotificationConsumerEnabled = new cdk.CfnParameter(scope, "WatchlistNotificationConsumerEnabled", {
    type: "String",
    default: "false",
    allowedValues: ["true", "false"],
    description: "Enable the dedicated watchlist notification SQS Lambda after the native-consumer handoff is complete.",
  });
  const watchlistNotificationConsumerActivation = new cdk.CfnCondition(scope, "WatchlistNotificationConsumerActivation", {
    expression: cdk.Fn.conditionEquals(watchlistNotificationConsumerEnabled.valueAsString, "true"),
  });
  const notificationDeliveryConsumerEnabled = new cdk.CfnParameter(scope, "NotificationDeliveryConsumerEnabled", {
    type: "String",
    default: "false",
    allowedValues: ["true", "false"],
    description: "Enable the dedicated notification-delivery SQS Lambda only after the native consumer is stopped, SES recipients are approved, and delivery recovery gates pass.",
  });
  const notificationDeliveryConsumerActivation = new cdk.CfnCondition(scope, "NotificationDeliveryConsumerActivation", {
    expression: cdk.Fn.conditionEquals(notificationDeliveryConsumerEnabled.valueAsString, "true"),
  });
  const cdcRouterActivation = includeCdcRouterActivation
    ? cdcRouterCondition(scope)
    : undefined;

  return {
    commitSha,
    productListingOpenSearchConsumerActivation,
    productListingNormalizationConsumerActivation,
    productContentAssessmentConsumerActivation,
    productEmbeddingConsumerActivation,
    searchFilterProjectionConsumerActivation,
    searchFilterPercolatorConsumerActivation,
    searchFilterMatchNotificationConsumerActivation,
    watchlistNotificationConsumerActivation,
    notificationDeliveryConsumerActivation,
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
