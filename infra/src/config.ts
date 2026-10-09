import * as cdk from "aws-cdk-lib";
import { WORKER_QUEUE_SETTINGS, type WorkerQueueSettings } from "./worker-queue-config";

export const STAGES = ["prod", "dev"] as const;
export type StageName = (typeof STAGES)[number];

export const ARTIFACT_BUCKET_NAME = "aura-historia-binary-artifacts-eu-central-1";
export const MAIL_TEMPLATE_BUCKET_NAME = "aura-historia-mail-templates-eu-central-1";
export const CLOUDFORMATION_STAGING_BUCKET_NAME = "aura-historia-cfn-artifcats-eu-central-1";
export const WORKLOAD_REGION = "eu-central-1";
export const DMS_CDC_INITIAL_START_POSITION_PARAMETER_ID = "InitialCdcStartPosition";
export const DMS_CDC_INITIAL_START_POSITION_PARAMETER_LOGICAL_ID = "DmsCdcInitialCdcStartPosition";
export const DMS_CDC_INITIAL_START_POSITION_PATTERN = "^$|^[0-9A-F]{1,8}/[0-9A-F]{1,8}$";
export const DMS_CDC_INITIAL_START_POSITION_CONSTRAINT = "must be empty or an uppercase PostgreSQL LSN in X/Y hexadecimal format";

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

export interface CognitoEmailConfig {
  readonly configurationSet: string;
  readonly from: string;
  readonly identityDomain: string;
  readonly replyTo: string;
}

export interface CognitoGoogleIdentityProviderConfig {
  readonly kind: "google";
  readonly providerName: "Google";
  readonly clientIdParameterName: string;
  readonly clientSecretParameterName: string;
  readonly scopes: readonly string[];
  readonly existingEmailAction: "LINK_VERIFIED";
  readonly linkSourceAttributeName: "Cognito_Subject";
}

export interface CognitoFacebookIdentityProviderConfig {
  readonly kind: "facebook";
  readonly providerName: "Facebook";
  readonly apiVersion: "v21.0";
  readonly clientIdParameterName: string;
  readonly clientSecretParameterName: string;
  readonly scopes: readonly string[];
  readonly existingEmailAction: "REJECT";
}

// Each provider explicitly defines credentials, scopes, attribute mappings, client support, and collision policy.
export type CognitoIdentityProviderConfig =
  | CognitoGoogleIdentityProviderConfig
  | CognitoFacebookIdentityProviderConfig;

export interface NotificationEmailConfig {
  readonly configurationSet: string;
  readonly from: string;
  readonly identityDomain: string;
  readonly replyTo: string;
}

export interface NetworkConfig {
  readonly cidr: string;
  readonly region: typeof WORKLOAD_REGION;
  readonly stageOpenSearchEgressCidrs: readonly string[];
}

export interface RdsConfig {
  readonly databaseName: string;
  readonly engineVersion: "16.13";
  readonly instanceType: string;
  readonly allocatedStorageGiB: number;
  readonly maxAllocatedStorageGiB: number;
  readonly backupRetentionDays: number;
}

export interface DmsConfig {
  readonly engineVersion: "3.6.1";
  readonly replicationInstanceClass: "dms.t3.small";
  readonly initialCdcStartPositionParameterId: typeof DMS_CDC_INITIAL_START_POSITION_PARAMETER_ID;

  readonly lobMaxSizeKiB: 512;
  readonly kinesisRetentionDays: 7;
}

export interface SearchFilterClassifierConfig {
  readonly provider: "cloudflare";
  readonly model: "clef-flash" | "clef";
  readonly shouldShowThresholdBps: number;
}

const SEARCH_FILTER_CLASSIFIER_CONFIG: Record<StageName, SearchFilterClassifierConfig> = {
  prod: { provider: "cloudflare", model: "clef-flash", shouldShowThresholdBps: 5_000 },
  dev: { provider: "cloudflare", model: "clef-flash", shouldShowThresholdBps: 5_000 },
};

export interface StageConfig {
  readonly stage: StageName;
  readonly isProd: boolean;
  readonly network: NetworkConfig | undefined;
  readonly rds: RdsConfig | undefined;
  readonly dms: DmsConfig | undefined;
  readonly searchFilterClassifier: SearchFilterClassifierConfig;
  readonly removalPolicy: cdk.RemovalPolicy;
  readonly workerQueues: WorkerQueueSettings;
  readonly apiEndpointUrl: string | undefined;
  readonly apiDomainName: string | undefined;
  readonly apiGatewayCertificateArn: string | undefined;
  readonly apiCloudFrontCertificateArn: string | undefined;
  readonly apiCloudFrontAliases: string[];
  readonly apiCorsAllowOrigins: string[];
  readonly cognitoCallbackUrls: string[];
  readonly cognitoLogoutUrls: string[];
  readonly cognitoIdentityProviders: readonly CognitoIdentityProviderConfig[];
  readonly cognitoEmail: CognitoEmailConfig | undefined;
  readonly notificationEmail: NotificationEmailConfig;
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
}

export function isStageName(value: string): value is StageName {
  return (STAGES as readonly string[]).includes(value);
}

export function stageConfig(stage: StageName): StageConfig {
  if (!isStageName(stage)) throw new Error(`Unsupported CDK stage '${stage}'.`);
  const isProd = stage === "prod";
  const emailConfigurationSetName = `aura-historia-${stage}-email`;

  const apiDomainName = isProd ? "api.aura-historia.com" : "api.stage.aura-historia.com";
  const apiCloudFrontAliases = [apiDomainName];

  return {
    stage,
    isProd,
    network: {
      cidr: isProd ? "10.64.0.0/16" : "10.65.0.0/16",
      region: WORKLOAD_REGION,
      // Reviewed stage host A record; confirm ownership and DNS before deploying changes.
      stageOpenSearchEgressCidrs: stage === "dev" ? ["148.251.91.20/32"] : [],
    },
    rds: {
      databaseName: "aura_historia",
      engineVersion: "16.13",
      instanceType: isProd ? "t4g.medium" : "t4g.small",
      allocatedStorageGiB: isProd ? 50 : 30,
      maxAllocatedStorageGiB: isProd ? 100 : 60,
      backupRetentionDays: isProd ? 14 : 7,
    },
    dms: {
      engineVersion: "3.6.1",
      replicationInstanceClass: "dms.t3.small",
      initialCdcStartPositionParameterId: DMS_CDC_INITIAL_START_POSITION_PARAMETER_ID,
      lobMaxSizeKiB: 512,
      kinesisRetentionDays: 7,
    },
    searchFilterClassifier: SEARCH_FILTER_CLASSIFIER_CONFIG[stage],
    removalPolicy: isProd ? cdk.RemovalPolicy.RETAIN : cdk.RemovalPolicy.DESTROY,
    workerQueues: WORKER_QUEUE_SETTINGS,
    apiEndpointUrl: `https://${apiDomainName}`,
    apiDomainName,
    apiGatewayCertificateArn: ssmValue(`/certificates/${stage}/api-regional-certificate-arn`),
    apiCloudFrontCertificateArn: ssmValue(`/certificates/${stage}/api-cloudfront-certificate-arn`),
    apiCloudFrontAliases,
    apiCorsAllowOrigins: isProd ? [...PROD_API_CORS_ALLOW_ORIGINS] : ["*"],
    cognitoCallbackUrls: isProd ? [PROD_FRONTEND_URL] : [LOCALHOST_CALLBACK_URL, STAGE_FRONTEND_URL],
    cognitoLogoutUrls: isProd ? [PROD_FRONTEND_URL] : [LOCALHOST_CALLBACK_URL, STAGE_FRONTEND_URL],
    cognitoIdentityProviders: [
      {
        kind: "google",
        providerName: "Google",
        clientIdParameterName: `/cognito/${stage}/identity-providers/google/client-id`,
        clientSecretParameterName: `/cognito/${stage}/identity-providers/google/client-secret`,
        scopes: ["openid", "email", "profile"],
        existingEmailAction: "LINK_VERIFIED",
        linkSourceAttributeName: "Cognito_Subject",
      },
      {
        kind: "facebook",
        providerName: "Facebook",
        apiVersion: "v21.0",
        clientIdParameterName: `/cognito/${stage}/identity-providers/facebook/client-id`,
        clientSecretParameterName: `/cognito/${stage}/identity-providers/facebook/client-secret`,
        scopes: ["email", "public_profile"],
        existingEmailAction: "REJECT",
      },
    ],
    cognitoEmail: {
      configurationSet: emailConfigurationSetName,
      from: "Aura Historia <auth@notify.aura-historia.com>",
      identityDomain: "notify.aura-historia.com",
      replyTo: "contact@aura-historia.com",
    },
    notificationEmail: {
      configurationSet: emailConfigurationSetName,
      from: ssmValue(`/notifications/${stage}/email-from`),
      identityDomain: "notify.aura-historia.com",
      replyTo: ssmValue(`/notifications/${stage}/email-reply-to`),
    },
    opensearchDomainName: `aura-historia-${stage}`,
    opensearchEndpointUrl: ssmValue(`/opensearch/${stage}/endpoint-url`),
    enableProductionObservability: isProd,
    stripeCheckoutCancelUrl: isProd ? "https://aura-historia.com" : "https://stage.aura-historia.com",
    stripeCheckoutSuccessUrl: isProd
      ? "https://aura-historia.com/me/account"
      : "https://stage.aura-historia.com/me/account",
    stripePortalReturnUrl: isProd
      ? "https://aura-historia.com/me/account"
      : "https://stage.aura-historia.com/me/account",
    stripeEventBusName: ssmValue(`/eventbridge/${stage}/stripe-event-bus-name`),
    shopifyEventBusName: ssmValue(`/eventbridge/${stage}/shopify-event-bus-name`),
    stripeProProductId: ssmValue(`/stripe/${stage}/pro-product-id`),
    stripeUltimateProductId: ssmValue(`/stripe/${stage}/ultimate-product-id`),
    stripeProMonthlyPriceId: ssmValue(`/stripe/${stage}/pro-monthly-price-id`),
    stripeProYearlyPriceId: ssmValue(`/stripe/${stage}/pro-yearly-price-id`),
    stripeUltimateMonthlyPriceId: ssmValue(`/stripe/${stage}/ultimate-monthly-price-id`),
    stripeUltimateYearlyPriceId: ssmValue(`/stripe/${stage}/ultimate-yearly-price-id`),
  };
}

export function ssmValue(path: string): string {
  return `{{resolve:ssm:${path}}}`;
}
