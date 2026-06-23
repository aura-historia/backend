import * as cdk from "aws-cdk-lib";
import { Construct } from "constructs";
import type { StageConfig } from "./config";

export interface ApplicationParameters {
  readonly artifactBucketName: string;
  readonly resourceBucketName: string;
  readonly mailTemplateBucketName: string;
  readonly commitSha: string;
  readonly stage: string;
  readonly stageName: string;
  readonly localStackMappedPort: string;
  readonly opensearchEndpointUrl: string;
  readonly stripeEventBusName: string;
  readonly shopifyEventBusName: string;
  readonly stripeProProductId: string;
  readonly stripeUltimateProductId: string;
  readonly stripeProMonthlyPriceId: string;
  readonly stripeProYearlyPriceId: string;
  readonly stripeUltimateMonthlyPriceId: string;
  readonly stripeUltimateYearlyPriceId: string;
}

export function applicationParameters(scope: Construct, config: StageConfig): ApplicationParameters {
  const artifactBucket = new cdk.CfnParameter(scope, "ArtifactBucket", {
    type: "String",
    description: "S3 bucket containing pre-built Lambda ZIP artifacts.",
  });

  const resourceBucket = new cdk.CfnParameter(scope, "ResourceBucket", {
    type: "String",
    description: "S3 bucket for application resources kept for deploy-flow compatibility.",
    default: config.isEphemeral ? "aura-historia-resources-eu-central-1" : undefined,
  });

  const mailTemplateBucket = new cdk.CfnParameter(scope, "MailTemplateBucket", {
    type: "String",
    description: "S3 bucket containing compiled transactional mail templates.",
    default: config.isEphemeral ? "aura-historia-mail-templates-eu-central-1" : undefined,
  });

  const commitSha = new cdk.CfnParameter(scope, "CommitSHA", {
    type: "String",
    description: "Artifact version to deploy. Reusing an older SHA rolls back Lambda/template artifacts.",
  });

  const stage = new cdk.CfnParameter(scope, "Stage", {
    type: "String",
    description: "Runtime stage injected into Lambda functions.",
    default: config.stage,
    allowedValues: ["prod", "dev", "ephemeral"],
  });

  const stageName = new cdk.CfnParameter(scope, "StageName", {
    type: "String",
    description: "Physical resource name suffix and API Gateway stage name.",
    default: config.defaultStageName,
  });

  const localStackMappedPort = new cdk.CfnParameter(scope, "LocalStackMappedPort", {
    type: "String",
    description: "Host-mapped LocalStack edge port used by Lambdas running in LocalStack containers.",
    default: "4566",
  });

  const opensearchEndpointUrl = new cdk.CfnParameter(scope, "OpenSearchEndpointUrl", {
    type: "String",
    description: "Externally managed OpenSearch endpoint for dev/prod.",
    default: config.isEphemeral ? "" : undefined,
  });

  const stripeEventBusName = new cdk.CfnParameter(scope, "StripeEventBusName", {
    type: "String",
    description: "AWS EventBridge partner event bus name connected to Stripe.",
    default: config.isEphemeral ? "stripe-event-bus-ephemeral" : undefined,
  });

  const shopifyEventBusName = new cdk.CfnParameter(scope, "ShopifyEventBusName", {
    type: "String",
    description: "AWS EventBridge partner event bus name connected to Shopify.",
    default: config.isEphemeral ? "shopify-event-bus-ephemeral" : undefined,
  });

  const stripeProProductId = new cdk.CfnParameter(scope, "StripeProProductId", {
    type: "String",
    default: config.isEphemeral ? "prod_test_pro" : undefined,
  });

  const stripeUltimateProductId = new cdk.CfnParameter(scope, "StripeUltimateProductId", {
    type: "String",
    default: config.isEphemeral ? "prod_test_ultimate" : undefined,
  });

  const stripeProMonthlyPriceId = new cdk.CfnParameter(scope, "StripeProMonthlyPriceId", {
    type: "String",
    default: config.isEphemeral ? "price_pro_monthly_mock" : undefined,
  });

  const stripeProYearlyPriceId = new cdk.CfnParameter(scope, "StripeProYearlyPriceId", {
    type: "String",
    default: config.isEphemeral ? "price_pro_yearly_mock" : undefined,
  });

  const stripeUltimateMonthlyPriceId = new cdk.CfnParameter(scope, "StripeUltimateMonthlyPriceId", {
    type: "String",
    default: config.isEphemeral ? "price_ultimate_monthly_mock" : undefined,
  });

  const stripeUltimateYearlyPriceId = new cdk.CfnParameter(scope, "StripeUltimateYearlyPriceId", {
    type: "String",
    default: config.isEphemeral ? "price_ultimate_yearly_mock" : undefined,
  });

  return {
    artifactBucketName: artifactBucket.valueAsString,
    resourceBucketName: resourceBucket.valueAsString,
    mailTemplateBucketName: mailTemplateBucket.valueAsString,
    commitSha: commitSha.valueAsString,
    stage: stage.valueAsString,
    stageName: stageName.valueAsString,
    localStackMappedPort: localStackMappedPort.valueAsString,
    opensearchEndpointUrl: opensearchEndpointUrl.valueAsString,
    stripeEventBusName: stripeEventBusName.valueAsString,
    shopifyEventBusName: shopifyEventBusName.valueAsString,
    stripeProProductId: stripeProProductId.valueAsString,
    stripeUltimateProductId: stripeUltimateProductId.valueAsString,
    stripeProMonthlyPriceId: stripeProMonthlyPriceId.valueAsString,
    stripeProYearlyPriceId: stripeProYearlyPriceId.valueAsString,
    stripeUltimateMonthlyPriceId: stripeUltimateMonthlyPriceId.valueAsString,
    stripeUltimateYearlyPriceId: stripeUltimateYearlyPriceId.valueAsString,
  };
}
