import type * as ses from "aws-cdk-lib/aws-ses";
import { MAIL_TEMPLATE_BUCKET_NAME, ssmValue, type StageConfig } from "../config";
import type { Search } from "./opensearch";
import type { QueueCatalog } from "./queues";
import type { PostgresConnectionSettings } from "./storage";

export interface LambdaEnvironmentContext {
  readonly config: StageConfig;
  readonly emailConfigurationSet: ses.IConfigurationSet;
  readonly commitSha: string;
  readonly postgres: PostgresConnectionSettings;
  readonly search: Search;
  readonly queues: QueueCatalog;
}

export function providerSignupEnvironment(context: LambdaEnvironmentContext): Record<string, string> {
  return {
    COGNITO_PROVIDER_SIGNUP_POLICY: JSON.stringify(
      context.config.cognitoIdentityProviders.map((provider) => ({
        providerName: provider.providerName,
        existingEmailAction: provider.existingEmailAction,
        ...(provider.existingEmailAction === "LINK_VERIFIED"
          ? { linkSourceAttributeName: provider.linkSourceAttributeName }
          : {}),
      })),
    ),
  };
}

/** Web app origin that emails link to. */
function frontendOrigin(config: StageConfig): string {
  return config.isProd ? "https://aura-historia.com" : "https://stage.aura-historia.com";
}

export function notificationDeliveryEnvironment(context: LambdaEnvironmentContext): Record<string, string> {
  return {
    COMMIT_SHA: context.commitSha,
    NOTIFICATION_EMAIL_CONFIGURATION_SET: context.emailConfigurationSet.configurationSetName,
    NOTIFICATION_EMAIL_FROM: context.config.notificationEmail.from,
    NOTIFICATION_EMAIL_FRONTEND_ORIGIN: frontendOrigin(context.config),
    NOTIFICATION_EMAIL_REPLY_TO: context.config.notificationEmail.replyTo,
    S3_BUCKET_NAME_TEMPLATES: MAIL_TEMPLATE_BUCKET_NAME,
    STAGE: context.config.stage,
  };
}

export function marketingConsentSyncEnvironment(context: LambdaEnvironmentContext): Record<string, string> {
  return loopsNewsletterEnvironment(context.config);
}

function loopsNewsletterEnvironment(config: StageConfig): Record<string, string> {
  return {
    LOOPS_API_BASE_URL: "https://app.loops.so/api",
    LOOPS_API_KEY: ssmValue(`/loops/${config.stage}/api-key`),
    LOOPS_NEWSLETTER_LIST_ID: ssmValue(`/loops/${config.stage}/newsletter-list-id`),
  };
}

export function mcpEnvironment(context: LambdaEnvironmentContext): Record<string, string> {
  return {
    AWS_LAMBDA_HTTP_IGNORE_STAGE_IN_PATH: "true",
    COMMIT_SHA: context.commitSha,
    MCP_ALLOWED_HOSTS: context.config.mcp.domainName,
    MCP_ALLOWED_ORIGINS: context.config.mcp.allowedOrigins.join(","),
  };
}

export function apiEnvironment(context: LambdaEnvironmentContext): Record<string, string> {
  const { config, search } = context;
  const environment = {
    AWS_LAMBDA_HTTP_IGNORE_STAGE_IN_PATH: "true",
    COMMIT_SHA: context.commitSha,
    OPENSEARCH_ENDPOINT_URL: search.endpointUrl,
    PRODUCT_LISTING_INGESTION_QUEUE_URL: context.queues.productListingIngestion.queue.queueUrl,
    NEWSLETTER_CONFIRMATION_EMAIL_CONFIGURATION_SET: context.emailConfigurationSet.configurationSetName,
    NEWSLETTER_CONFIRMATION_EMAIL_FROM: config.notificationEmail.from,
    NEWSLETTER_CONFIRMATION_EMAIL_REPLY_TO: config.notificationEmail.replyTo,
    NEWSLETTER_CONFIRMATION_FRONTEND_ORIGIN: frontendOrigin(config),
    S3_BUCKET_NAME_TEMPLATES: MAIL_TEMPLATE_BUCKET_NAME,
    STAGE: config.stage,
    STRIPE_CHECKOUT_CANCEL_URL: config.stripeCheckoutCancelUrl,
    STRIPE_CHECKOUT_SUCCESS_URL: config.stripeCheckoutSuccessUrl,
    STRIPE_PORTAL_RETURN_URL: config.stripePortalReturnUrl,
    STRIPE_PRO_MONTHLY_PRICE_ID: config.stripeProMonthlyPriceId,
    STRIPE_PRO_YEARLY_PRICE_ID: config.stripeProYearlyPriceId,
    STRIPE_ULTIMATE_MONTHLY_PRICE_ID: config.stripeUltimateMonthlyPriceId,
    STRIPE_ULTIMATE_YEARLY_PRICE_ID: config.stripeUltimateYearlyPriceId,
  };


  return {
    ...environment,
    OPENSEARCH_PASSWORD: ssmValue(`/opensearch/${config.stage}/reader/password`),
    OPENSEARCH_USERNAME: ssmValue(`/opensearch/${config.stage}/reader/username`),
    AURA_HISTORIA_GOOGLE_ADC_CREDENTIALS_JSON: ssmValue(
      `/secrets/${config.stage}/google-application-credentials`,
    ),
    STRIPE_API_KEY: ssmValue(`/stripe/${config.stage}/api-key`),
    VERTEX_AI_LOCATION: ssmValue(`/vertex-ai/${config.stage}/location`),
    VERTEX_AI_PROJECT_ID: ssmValue(`/vertex-ai/${config.stage}/project-id`),
    ...loopsNewsletterEnvironment(config),
    LOOPS_WEBHOOK_SIGNING_SECRET: ssmValue(`/loops/${config.stage}/webhook-signing-secret`),
  };
}
