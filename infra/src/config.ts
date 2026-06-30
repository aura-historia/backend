import * as cdk from "aws-cdk-lib";

export const STAGES = ["prod", "dev", "ephemeral"] as const;
export type StageName = (typeof STAGES)[number];

export const ARTIFACT_BUCKET_NAME = "aura-historia-binary-artifacts-eu-central-1";
export const MAIL_TEMPLATE_BUCKET_NAME = "aura-historia-mail-templates-eu-central-1";
export const CLOUDFORMATION_STAGING_BUCKET_NAME = "aura-historia-cfn-artifcats-eu-central-1";

const LOCALHOST_CALLBACK_URL = "http://localhost:3000";
const STAGE_FRONTEND_URL = "https://stage.aura-historia.com/";
const PROD_FRONTEND_URL = "https://aura-historia.com/";

const PROD_API_CORS_ALLOW_ORIGINS = [
  "https://aura-historia.com",
  "https://admin.shopify.com",
  "https://partners.shopify.com",
  "https://shopify.com",
  "https://*.myshopify.com",
] as const;

export interface StageConfig {
  readonly stage: StageName;
  readonly isProd: boolean;
  readonly isEphemeral: boolean;
  readonly removalPolicy: cdk.RemovalPolicy;
  readonly apiEndpointUrl: string | undefined;
  readonly apiCorsAllowOrigins: string[];
  readonly cognitoCallbackUrls: string[];
  readonly cognitoLogoutUrls: string[];
  readonly opensearchDomainName: string;
  readonly opensearchEndpointUrl: string;
  readonly enableProductionObservability: boolean;
  readonly stripeCheckoutCancelUrl: string;
  readonly stripeCheckoutSuccessUrl: string;
  readonly stripePortalReturnUrl: string;
  readonly stripeEventBusName: string;
  readonly shopifyEventBusName: string;
  readonly stripeProProductId: string;
  readonly stripeUltimateProductId: string;
  readonly stripeProMonthlyPriceId: string;
  readonly stripeProYearlyPriceId: string;
  readonly stripeUltimateMonthlyPriceId: string;
  readonly stripeUltimateYearlyPriceId: string;
  readonly localStackMappedPort: string;
}

export interface StageConfigOptions {
  readonly localStackMappedPort?: string;
}

export function isStageName(value: string): value is StageName {
  return (STAGES as readonly string[]).includes(value);
}

export function stageConfig(stage: StageName, options: StageConfigOptions = {}): StageConfig {
  const isProd = stage === "prod";
  const isEphemeral = stage === "ephemeral";

  return {
    stage,
    isProd,
    isEphemeral,
    removalPolicy: isProd ? cdk.RemovalPolicy.RETAIN : cdk.RemovalPolicy.DESTROY,
    apiEndpointUrl:
      stage === "prod"
        ? "https://api.aura-historia.com"
        : stage === "dev"
          ? "https://api.dev.aura-historia.com"
          : undefined,
    apiCorsAllowOrigins: isProd ? [...PROD_API_CORS_ALLOW_ORIGINS] : ["*"],
    cognitoCallbackUrls:
      stage === "prod"
        ? [PROD_FRONTEND_URL]
        : stage === "dev"
          ? [LOCALHOST_CALLBACK_URL, STAGE_FRONTEND_URL]
          : [LOCALHOST_CALLBACK_URL],
    cognitoLogoutUrls:
      stage === "prod"
        ? [PROD_FRONTEND_URL]
        : stage === "dev"
          ? [LOCALHOST_CALLBACK_URL, STAGE_FRONTEND_URL]
          : [LOCALHOST_CALLBACK_URL],
    opensearchDomainName: isEphemeral ? "test-domain" : `aura-historia-${stage}`,
    opensearchEndpointUrl: isEphemeral ? "" : ssmValue(`/opensearch/${stage}/endpoint-url`),
    enableProductionObservability: isProd,
    stripeCheckoutCancelUrl: isProd ? "https://aura-historia.com" : "https://stage.aura-historia.com",
    stripeCheckoutSuccessUrl: isProd
      ? "https://aura-historia.com/me/account"
      : "https://stage.aura-historia.com/me/account",
    stripePortalReturnUrl: isProd
      ? "https://aura-historia.com/me/account"
      : "https://stage.aura-historia.com/me/account",
    stripeEventBusName: isEphemeral
      ? "stripe-event-bus-ephemeral"
      : ssmValue(`/eventbridge/${stage}/stripe-event-bus-name`),
    shopifyEventBusName: isEphemeral
      ? "shopify-event-bus-ephemeral"
      : ssmValue(`/eventbridge/${stage}/shopify-event-bus-name`),
    stripeProProductId: isEphemeral ? "prod_test_pro" : ssmValue(`/stripe/${stage}/pro-product-id`),
    stripeUltimateProductId: isEphemeral ? "prod_test_ultimate" : ssmValue(`/stripe/${stage}/ultimate-product-id`),
    stripeProMonthlyPriceId: isEphemeral ? "price_pro_monthly_mock" : ssmValue(`/stripe/${stage}/pro-monthly-price-id`),
    stripeProYearlyPriceId: isEphemeral ? "price_pro_yearly_mock" : ssmValue(`/stripe/${stage}/pro-yearly-price-id`),
    stripeUltimateMonthlyPriceId: isEphemeral
      ? "price_ultimate_monthly_mock"
      : ssmValue(`/stripe/${stage}/ultimate-monthly-price-id`),
    stripeUltimateYearlyPriceId: isEphemeral
      ? "price_ultimate_yearly_mock"
      : ssmValue(`/stripe/${stage}/ultimate-yearly-price-id`),
    localStackMappedPort: options.localStackMappedPort ?? "4566",
  };
}

export function ssmValue(path: string): string {
  return `{{resolve:ssm:${path}}}`;
}
