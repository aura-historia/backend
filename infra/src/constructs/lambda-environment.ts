import type * as ses from "aws-cdk-lib/aws-ses";
import { ARTIFACT_BUCKET_NAME, ssmValue, validatedMuseChallenge, type McpConfig, type StageConfig } from "../config";
import { credentialSecrets, type CredentialResolver } from "../credentials";
import type { Search } from "./opensearch";
import type { QueueCatalog } from "./queues";
import type { PostgresConnectionSettings } from "./storage";

/** Deployment parameters that saved-search previews share with scheduled matching. */
export interface SearchFilterClassifierParameters {
  readonly model: string;
  readonly shouldShowThresholdBps: string;
}

export interface LambdaEnvironmentContext {
  readonly config: StageConfig;
  readonly emailConfigurationSet: ses.IConfigurationSet;
  readonly commitSha: string;
  readonly mailTemplatePrefix: string;
  readonly searchFilterClassifier: SearchFilterClassifierParameters;
  readonly postgres: PostgresConnectionSettings;
  readonly search: Search;
  readonly queues: QueueCatalog;
  readonly credential: CredentialResolver;
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
    S3_BUCKET_NAME_TEMPLATES: ARTIFACT_BUCKET_NAME,
    MAIL_TEMPLATE_PREFIX: context.mailTemplatePrefix,
    STAGE: context.config.stage,
  };
}

export function marketingConsentSyncEnvironment(context: LambdaEnvironmentContext): Record<string, string> {
  return loopsNewsletterEnvironment(context);
}

function loopsNewsletterEnvironment(context: LambdaEnvironmentContext): Record<string, string> {
  const { config } = context;
  return {
    LOOPS_API_BASE_URL: "https://app.loops.so/api",
    LOOPS_API_KEY: context.credential(credentialSecrets(config.stage).loops, "apiKey"),
    LOOPS_NEWSLETTER_LIST_ID: ssmValue(`/loops/${config.stage}/newsletter-list-id`),
  };
}

export function mcpEnvironment(context: LambdaEnvironmentContext): Record<string, string> {
  return {
    ...listingProviderEnvironment(context),
    ...searchIntentTranslatorEnvironment(context),
    AWS_LAMBDA_HTTP_IGNORE_STAGE_IN_PATH: "true",
    COMMIT_SHA: context.commitSha,
    OAUTH_ISSUER_URL: context.config.oauthIssuerUrl,
    MCP_ALLOWED_HOSTS: context.config.mcp.domainName,
    MCP_ALLOWED_ORIGINS: context.config.mcp.allowedOrigins.join(","),
    ...openAiAppsChallengeEnvironment(context.config.mcp),
    ...museChallengeEnvironment(context.config.mcp),
  };
}

/** The MCP server refuses to start with an invalid token, so synthesis rejects it first. */
export function openAiAppsChallengeEnvironment(mcp: McpConfig): Record<string, string> {
  const token = mcp.openAiAppsChallenge;
  if (token === undefined) return {};
  if (!/^[\x21-\x7e]{1,512}$/.test(token)) {
    throw new Error("mcp.openAiAppsChallenge must be one visible ASCII token of at most 512 characters.");
  }
  return { MCP_OPENAI_APPS_CHALLENGE: token };
}

/** The MCP server refuses to start with an invalid token, so synthesis rejects it first. */
export function museChallengeEnvironment(mcp: McpConfig): Record<string, string> {
  const token = validatedMuseChallenge(mcp);
  return token === undefined ? {} : { MCP_MUSE_CHALLENGE: token };
}

export function apiEnvironment(context: LambdaEnvironmentContext): Record<string, string> {
  const { config } = context;
  const environment = {
    AWS_LAMBDA_HTTP_IGNORE_STAGE_IN_PATH: "true",
    COMMIT_SHA: context.commitSha,
    OAUTH_ISSUER_URL: config.oauthIssuerUrl,
    OAUTH_METADATA_DOCUMENT_CLIENTS_ENABLED: String(config.oauthMetadataDocumentClients.enabled),
    OAUTH_METADATA_DOCUMENT_ACCESS_TOKEN_TTL_SECONDS: String(config.oauthMetadataDocumentClients.accessTokenTtlSeconds),
    OAUTH_REFRESH_TOKEN_IDLE_TTL_SECONDS: String(config.oauthMetadataDocumentClients.refreshTokenIdleTtlSeconds),
    OAUTH_REFRESH_TOKEN_ABSOLUTE_TTL_SECONDS: String(config.oauthMetadataDocumentClients.refreshTokenAbsoluteTtlSeconds),
    ...(config.oauthMetadataDocumentClients.deniedHosts.length > 0
      ? { OAUTH_METADATA_DOCUMENT_DENIED_HOSTS: config.oauthMetadataDocumentClients.deniedHosts.join(",") }
      : {}),
    PRODUCT_LISTING_INGESTION_QUEUE_URL: context.queues.productListingIngestion.queue.queueUrl,
    NEWSLETTER_CONFIRMATION_EMAIL_CONFIGURATION_SET: context.emailConfigurationSet.configurationSetName,
    NEWSLETTER_CONFIRMATION_EMAIL_FROM: config.notificationEmail.from,
    NEWSLETTER_CONFIRMATION_EMAIL_REPLY_TO: config.notificationEmail.replyTo,
    NEWSLETTER_CONFIRMATION_FRONTEND_ORIGIN: frontendOrigin(config),
    S3_BUCKET_NAME_TEMPLATES: ARTIFACT_BUCKET_NAME,
    MAIL_TEMPLATE_PREFIX: context.mailTemplatePrefix,
    STRIPE_CHECKOUT_CANCEL_URL: config.stripeCheckoutCancelUrl,
    STRIPE_CHECKOUT_SUCCESS_URL: config.stripeCheckoutSuccessUrl,
    STRIPE_PORTAL_RETURN_URL: config.stripePortalReturnUrl,
    STRIPE_PRO_MONTHLY_PRICE_ID: config.stripeProMonthlyPriceId,
    STRIPE_PRO_YEARLY_PRICE_ID: config.stripeProYearlyPriceId,
    STRIPE_ULTIMATE_MONTHLY_PRICE_ID: config.stripeUltimateMonthlyPriceId,
    STRIPE_ULTIMATE_YEARLY_PRICE_ID: config.stripeUltimateYearlyPriceId,
  };


  const secrets = credentialSecrets(config.stage);
  return {
    ...environment,
    ...listingProviderEnvironment(context),
    ...searchFilterPreviewEnvironment(context),
    ...searchIntentTranslatorEnvironment(context),
    STRIPE_API_KEY: context.credential(secrets.stripe, "apiKey"),
    ...loopsNewsletterEnvironment(context),
    LOOPS_WEBHOOK_SIGNING_SECRET: context.credential(secrets.loops, "webhookSigningSecret"),
  };
}

/** Search-filter previews classify with the periodic matcher's model and threshold, one candidate per request. */
function searchFilterPreviewEnvironment(context: LambdaEnvironmentContext): Record<string, string> {
  const { config, searchFilterClassifier } = context;
  return {
    CLASSIFIER_MODEL_PROVIDER: config.searchFilterClassifier.provider,
    CLASSIFIER_MODEL: searchFilterClassifier.model,
    SEARCH_FILTER_MATCH_SHOULD_SHOW_THRESHOLD_BPS: searchFilterClassifier.shouldShowThresholdBps,
    CLOUDFLARE_ACCOUNT_ID: ssmValue(`/cloudflare/${config.stage}/account-id`),
    CLOUDFLARE_API_TOKEN: context.credential(credentialSecrets(config.stage).cloudflare, "workersAiApiToken"),
  };
}

/** Saved-search writes and previews translate the user's intent with the listing-title translator's provider. */
function searchIntentTranslatorEnvironment(context: LambdaEnvironmentContext): Record<string, string> {
  return {
    ANTHROPIC_API_KEY: context.credential(credentialSecrets(context.config.stage).anthropic, "apiKey"),
  };
}

function listingProviderEnvironment(context: LambdaEnvironmentContext): Record<string, string> {
  const { config, search } = context;
  const secrets = credentialSecrets(config.stage);
  return {
    STAGE: config.stage,
    PRODUCT_LISTING_SEARCH_PARALLEL_ENRICHMENT_ENABLED: String(config.isProd),
    OPENSEARCH_ENDPOINT_URL: search.endpointUrl,
    OPENSEARCH_PASSWORD: context.credential(secrets.openSearch, "readerPassword"),
    OPENSEARCH_USERNAME: ssmValue(`/opensearch/${config.stage}/reader/username`),
    AURA_HISTORIA_GOOGLE_ADC_CREDENTIALS_JSON: context.credential(secrets.google, "applicationCredentials"),
    VERTEX_AI_LOCATION: ssmValue(`/vertex-ai/${config.stage}/location`),
    VERTEX_AI_PROJECT_ID: ssmValue(`/vertex-ai/${config.stage}/project-id`),
  };
}
