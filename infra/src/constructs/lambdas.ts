import * as cdk from "aws-cdk-lib";
import * as dynamodb from "aws-cdk-lib/aws-dynamodb";
import * as iam from "aws-cdk-lib/aws-iam";
import * as lambda from "aws-cdk-lib/aws-lambda";
import * as s3 from "aws-cdk-lib/aws-s3";
import { Construct } from "constructs";
import type { StageConfig, StageName } from "../config";
import { ssmValue } from "../config";
import type { ApplicationParameters } from "../parameters";
import type { Search } from "./opensearch";
import type { QueueCatalog } from "./queues";
import type { PostgresConnectionSettings } from "./storage";

interface LambdaEnvironmentContext {
  readonly config: StageConfig;
  readonly commitSha: string;
  readonly mailTemplateBucket: s3.IBucket;
  readonly table: dynamodb.Table;
  readonly postgres: PostgresConnectionSettings;
  readonly queues: QueueCatalog;
  readonly search: Search;
}

interface LambdaDefinition {
  readonly id: string;
  readonly binaryName: string;
  readonly memorySize: number;
  readonly timeoutSeconds: number;
  readonly skipEphemeral?: boolean;
  readonly postgres?: boolean;
  readonly environment?: (context: LambdaEnvironmentContext) => Record<string, string>;
}

function defineLambdaDefinitions<T extends Record<string, LambdaDefinition>>(definitions: T): T {
  return definitions;
}

const LAMBDA_DEFINITIONS = defineLambdaDefinitions({
  cloudWatchLogRetention: {
    id: "CloudWatchLogRetentionLambda",
    binaryName: "cloudwatch-log-retention-lambda",
    memorySize: 128,
    timeoutSeconds: 10,
    environment: () => ({}),
  },
  newsletterApi: {
    id: "NewsletterApiLambda",
    binaryName: "newsletter-api",
    memorySize: 256,
    timeoutSeconds: 10,
    environment: (context) => ({
      ...baseEnvironment(context),
      STAGE: context.config.stage,
      ZOHO_ACCOUNTS_URL: secretOrTest(context.config, "zoho-accounts-url", "https://accounts.zoho.eu"),
      ZOHO_CAMPAIGNS_URL: secretOrTest(context.config, "zoho-campaigns-url", "https://campaigns.zoho.eu"),
      ZOHO_CLIENT_ID: secretOrTest(context.config, "zoho-client-id", "test-zoho-client-id"),
      ZOHO_CLIENT_SECRET: secretOrTest(context.config, "zoho-client-secret", "test-zoho-client-secret"),
      ZOHO_LIST_KEY: secretOrTest(context.config, "zoho-list-key", "test-zoho-list-key"),
      ZOHO_REFRESH_TOKEN: secretOrTest(context.config, "zoho-refresh-token", "test-zoho-refresh-token"),
    }),
  },
  notificationApi: {
    id: "NotificationApiLambda",
    binaryName: "notification-api",
    memorySize: 256,
    timeoutSeconds: 10,
    environment: stageEnvironment,
  },
  notificationSend: {
    id: "NotificationSendLambda",
    binaryName: "notification-send",
    memorySize: 512,
    timeoutSeconds: 60,
    environment: mailTemplateEnvironment,
  },
  oauthApi: {
    id: "OAuthApiLambda",
    binaryName: "oauth-api",
    memorySize: 128,
    postgres: true,
    timeoutSeconds: 10,
    environment: (context) => withLocalStackPort(context.config, stageEnvironment(context)),
  },
  partnerShopApplicationApi: {
    id: "PartnerShopApplicationApiLambda",
    binaryName: "partner-shop-application-api",
    memorySize: 256,
    postgres: true,
    timeoutSeconds: 10,
    environment: stageEnvironment,
  },
  partnerShopApplicationWorkflow: {
    id: "PartnerShopApplicationStepFunctionLambda",
    binaryName: "partner-shop-application-lambda",
    memorySize: 256,
    postgres: true,
    timeoutSeconds: 30,
    environment: (context) => ({
      ...baseEnvironment(context),
      COMMIT_SHA: context.commitSha,
      GOOGLE_GEOCODING_API_KEY: secretOrTest(context.config, "google-geocoding-api-key", "test-key"),
      S3_BUCKET_NAME_TEMPLATES: context.mailTemplateBucket.bucketName,
      STAGE_NAME: context.config.stage,
    }),
  },
  postConfirmation: {
    id: "PrimaryUserPoolPostConfirmationLambda",
    binaryName: "cognito-post-confirmation",
    memorySize: 256,
    postgres: true,
    timeoutSeconds: 5,
  },
  productApi: {
    id: "ProductApiLambda",
    binaryName: "product-api",
    memorySize: 512,
    postgres: true,
    timeoutSeconds: 10,
    environment: (context) =>
      withLocalStackPort(
        context.config,
        withOpenSearchCredentials(context.config, {
          ...baseEnvironment(context),
          ...(context.config.isEphemeral
            ? { GEMINI_API_KEY: "test-key" }
            : { GOOGLE_APPLICATION_CREDENTIALS: ssmSecret(context.config, "google-application-credentials") }),
          OPENSEARCH_ENDPOINT_URL: context.search.endpointUrl,
          STAGE: context.config.stage,
        }),
      ),
  },
  productApiPartner: {
    id: "ProductApiPartnerLambda",
    binaryName: "product-api-partner",
    memorySize: 512,
    postgres: true,
    timeoutSeconds: 10,
    environment: (context) =>
      withLocalStackPort(
        context.config,
        withOpenSearchCredentials(context.config, {
          ...baseEnvironment(context),
          ASYNC_PRODUCT_COMMAND_QUEUE_URL: context.queues.productPartnerIngest.queue.queueUrl,
          ...(context.config.isEphemeral
            ? {}
            : {
                GEMINI_API_KEY: ssmSecret(context.config, "gemini-api-key"),
                GOOGLE_GEOCODING_API_KEY: ssmSecret(context.config, "google-geocoding-api-key"),
              }),
          OPENSEARCH_ENDPOINT_URL: context.search.endpointUrl,
          STAGE: context.config.stage,
        }),
      ),
  },
  productMaterializeOpenSearch: {
    id: "ProductMaterializeOpenSearchLambda",
    binaryName: "product-lambda-materialize-opensearch",
    memorySize: 512,
    postgres: true,
    timeoutSeconds: 60,
    environment: (context) =>
      withLocalStackPort(
        context.config,
        withOpenSearchCredentials(context.config, {
          ...baseEnvironment(context),
          OPENSEARCH_ENDPOINT_URL: context.search.endpointUrl,
          STAGE: context.config.stage,
        }),
      ),
  },
  productDeleteProduct: {
    id: "ProductDeleteProductLambda",
    binaryName: "product-lambda-delete-product",
    memorySize: 512,
    postgres: true,
    timeoutSeconds: 60,
    environment: (context) =>
      withLocalStackPort(
        context.config,
        withOpenSearchCredentials(context.config, {
          ...baseEnvironment(context),
          OPENSEARCH_ENDPOINT_URL: context.search.endpointUrl,
          STAGE: context.config.stage,
        }),
      ),
  },
  productPartnerIngest: {
    id: "ProductPartnerIngestLambda",
    binaryName: "product-lambda-ingest-partner-products",
    memorySize: 512,
    postgres: true,
    timeoutSeconds: 30,
    environment: (context) =>
      withLocalStackPort(
        context.config,
        withOpenSearchCredentials(context.config, {
          ...baseEnvironment(context),
          GEMINI_API_KEY: secretOrTest(context.config, "gemini-api-key", "test-key"),
          GOOGLE_GEOCODING_API_KEY: secretOrTest(context.config, "google-geocoding-api-key", "test-key"),
          OPENSEARCH_ENDPOINT_URL: context.search.endpointUrl,
          STAGE: context.config.stage,
        }),
      ),
  },
  productPipelineEmbedText: {
    id: "ProductPipelineEmbedTextLambda",
    binaryName: "product-pipeline-embed-text",
    memorySize: 512,
    timeoutSeconds: 60,
    environment: (context) =>
      context.config.isEphemeral
        ? { ...baseEnvironment(context), GEMINI_API_KEY: "test-key" }
        : { ...baseEnvironment(context), GOOGLE_APPLICATION_CREDENTIALS: ssmSecret(context.config, "google-application-credentials") },
  },
  productPipelineTranslate: {
    id: "ProductPipelineTranslateLambda",
    binaryName: "product-pipeline-translate",
    memorySize: 512,
    timeoutSeconds: 60,
    environment: (context) => ({
      ...baseEnvironment(context),
      GEMINI_API_KEY: secretOrTest(context.config, "gemini-api-key", "test-key"),
      STAGE: context.config.stage,
    }),
  },
  productUpdateNotifyUser: {
    id: "ProductUpdateNotifyUserLambda",
    binaryName: "product-lambda-update-notify-user",
    memorySize: 512,
    timeoutSeconds: 60,
    environment: mailTemplateEnvironment,
  },
  productWatchlistApi: {
    id: "ProductWatchlistApiLambda",
    binaryName: "product-watchlist-api",
    memorySize: 256,
    postgres: true,
    timeoutSeconds: 10,
    environment: stageEnvironment,
  },
  searchFilterApi: {
    id: "SearchFilterApiLambda",
    binaryName: "search-filter-api",
    memorySize: 512,
    postgres: true,
    timeoutSeconds: 10,
    environment: (context) =>
      withLocalStackPort(
        context.config,
        withOpenSearchCredentials(context.config, {
          ...baseEnvironment(context),
          GEMINI_API_KEY: secretOrTest(context.config, "gemini-api-key", "test-key"),
          ...(context.config.isEphemeral
            ? {}
            : { GOOGLE_APPLICATION_CREDENTIALS: ssmSecret(context.config, "google-application-credentials") }),
          OPENSEARCH_ENDPOINT_URL: context.search.endpointUrl,
          STAGE: context.config.stage,
        }),
      ),
  },
  searchFilterOpenSearchSync: {
    id: "SearchFilterOpenSearchSyncLambda",
    binaryName: "search-filter-lambda-opensearch-sync",
    memorySize: 512,
    postgres: true,
    timeoutSeconds: 60,
    environment: (context) =>
      withLocalStackPort(
        context.config,
        withOpenSearchCredentials(context.config, {
          ...baseEnvironment(context),
          OPENSEARCH_ENDPOINT_URL: context.search.endpointUrl,
          STAGE: context.config.stage,
        }),
      ),
  },
  searchFilterPercolateProduct: {
    id: "SearchFilterPercolateProductLambda",
    binaryName: "search-filter-lambda-percolate-product",
    memorySize: 512,
    postgres: true,
    timeoutSeconds: 60,
    environment: (context) =>
      withLocalStackPort(
        context.config,
        withOpenSearchCredentials(context.config, {
          ...baseEnvironment(context),
          COMMIT_SHA: context.commitSha,
          GEMINI_API_KEY: secretOrTest(context.config, "gemini-api-key", "test-key"),
          OPENSEARCH_ENDPOINT_URL: context.search.endpointUrl,
          S3_BUCKET_NAME_TEMPLATES: context.mailTemplateBucket.bucketName,
          STAGE_NAME: context.config.stage,
        }),
      ),
  },
  shopApi: {
    id: "ShopApiLambda",
    binaryName: "shop-api",
    memorySize: 512,
    postgres: true,
    timeoutSeconds: 10,
    environment: (context) =>
      withLocalStackPort(
        context.config,
        withOpenSearchCredentials(context.config, {
          ...baseEnvironment(context),
          GOOGLE_GEOCODING_API_KEY: secretOrTest(context.config, "google-geocoding-api-key", "test-key"),
          OPENSEARCH_ENDPOINT_URL: context.search.endpointUrl,
          STAGE: context.config.stage,
        }),
      ),
  },
  shopOpenSearchIndex: {
    id: "ShopOpenSearchIndexLambda",
    binaryName: "shop-lambda-opensearch-index",
    memorySize: 256,
    postgres: true,
    timeoutSeconds: 30,
    environment: openSearchWorkerEnvironment,
  },
  shopify: {
    id: "ShopifyLambda",
    binaryName: "shopify-lambda",
    memorySize: 256,
    postgres: true,
    timeoutSeconds: 30,
    environment: (context) =>
      withOpenSearchCredentials(context.config, {
        ...baseEnvironment(context),
        GEMINI_API_KEY: secretOrTest(context.config, "gemini-api-key", "test-key"),
        GOOGLE_GEOCODING_API_KEY: secretOrTest(context.config, "google-geocoding-api-key", "test-key"),
        OPENSEARCH_ENDPOINT_URL: context.search.endpointUrl,
      }),
  },
  stripeApi: {
    id: "StripeApiLambda",
    binaryName: "stripe-api",
    memorySize: 256,
    timeoutSeconds: 10,
    environment: (context) => ({
      ...baseEnvironment(context),
      STAGE: context.config.stage,
      ...(context.config.isEphemeral ? {} : { STRIPE_API_KEY: ssmSecret(context.config, "stripe-api-key") }),
      STRIPE_CHECKOUT_CANCEL_URL: context.config.stripeCheckoutCancelUrl,
      STRIPE_CHECKOUT_SUCCESS_URL: context.config.stripeCheckoutSuccessUrl,
      STRIPE_PORTAL_RETURN_URL: context.config.stripePortalReturnUrl,
      STRIPE_PRO_MONTHLY_PRICE_ID: context.config.stripeProMonthlyPriceId,
      STRIPE_PRO_YEARLY_PRICE_ID: context.config.stripeProYearlyPriceId,
      STRIPE_ULTIMATE_MONTHLY_PRICE_ID: context.config.stripeUltimateMonthlyPriceId,
      STRIPE_ULTIMATE_YEARLY_PRICE_ID: context.config.stripeUltimateYearlyPriceId,
    }),
  },
  stripe: {
    id: "StripeLambda",
    binaryName: "stripe-lambda",
    memorySize: 256,
    postgres: true,
    timeoutSeconds: 30,
    environment: (context) => ({
      ...baseEnvironment(context),
      STRIPE_PRO_PRODUCT_ID: context.config.stripeProProductId,
      STRIPE_ULTIMATE_PRODUCT_ID: context.config.stripeUltimateProductId,
    }),
  },
  userApi: {
    id: "UserApiLambda",
    binaryName: "user-api",
    memorySize: 256,
    postgres: true,
    timeoutSeconds: 10,
    environment: (context) =>
      withLocalStackPort(
        context.config,
        withOpenSearchCredentials(context.config, {
          ...baseEnvironment(context),
          ...(context.config.isEphemeral
            ? {}
            : { GOOGLE_GEOCODING_API_KEY: ssmSecret(context.config, "google-geocoding-api-key") }),
          OPENSEARCH_ENDPOINT_URL: context.search.endpointUrl,
          STAGE: context.config.stage,
        }),
      ),
  },
  userOpenSearchIndex: {
    id: "UserOpenSearchIndexLambda",
    binaryName: "user-lambda-index-opensearch",
    memorySize: 256,
    postgres: true,
    timeoutSeconds: 30,
    environment: openSearchWorkerEnvironment,
  },
  userTierUpdate: {
    id: "UserTierUpdateLambda",
    binaryName: "user-lambda-tier-update",
    memorySize: 256,
    postgres: true,
    timeoutSeconds: 30,
  },
  webhookApi: {
    id: "WebhookApiLambda",
    binaryName: "webhook-api",
    memorySize: 512,
    postgres: true,
    timeoutSeconds: 10,
    environment: (context) =>
      withLocalStackPort(
        context.config,
        withOpenSearchCredentials(context.config, {
          ...baseEnvironment(context),
          ASYNC_PRODUCT_COMMAND_QUEUE_URL: context.queues.productPartnerIngest.queue.queueUrl,
          ...(context.config.isEphemeral
            ? {}
            : {
                GEMINI_API_KEY: ssmSecret(context.config, "gemini-api-key"),
                GOOGLE_GEOCODING_API_KEY: ssmSecret(context.config, "google-geocoding-api-key"),
              }),
          OPENSEARCH_ENDPOINT_URL: context.search.endpointUrl,
          STAGE: context.config.stage,
        }),
      ),
  },
  fxRateSync: {
    id: "FxRateSyncLambda",
    binaryName: "fxrate-lambda",
    memorySize: 128,
    postgres: true,
    timeoutSeconds: 10,
    skipEphemeral: true,
    environment: () => ({
      FXRATES_API_TOKEN: ssmValue("/fxratesapi/prod/api-token"),
    }),
  },
} as const);

export type LambdaKey = keyof typeof LAMBDA_DEFINITIONS;
export type LambdaCatalog = Partial<Record<LambdaKey, lambda.IFunction>> &
  Record<Exclude<LambdaKey, "fxRateSync">, lambda.IFunction>;
export type LambdaFunctions = Partial<Record<LambdaKey, lambda.Function>> &
  Record<Exclude<LambdaKey, "fxRateSync">, lambda.Function>;

export interface LambdasProps {
  readonly config: StageConfig;
  readonly parameters: ApplicationParameters;
  readonly artifactBucket: s3.IBucket;
  readonly mailTemplateBucket: s3.IBucket;
  readonly table: dynamodb.Table;
  readonly postgres: PostgresConnectionSettings;
  readonly queues: QueueCatalog;
  readonly search: Search;
}

export class Lambdas extends Construct {
  readonly functions: LambdaFunctions;

  constructor(scope: Construct, id: string, props: LambdasProps) {
    super(scope, id);

    const functions = {} as Partial<Record<LambdaKey, lambda.Function>>;
    const environmentContext: LambdaEnvironmentContext = {
      config: props.config,
      commitSha: props.parameters.commitSha,
      mailTemplateBucket: props.mailTemplateBucket,
      table: props.table,
      postgres: props.postgres,
      queues: props.queues,
      search: props.search,
    };

    for (const [key, definition] of Object.entries(LAMBDA_DEFINITIONS) as [LambdaKey, LambdaDefinition][]) {
      if (props.config.isEphemeral && definition.skipEphemeral) {
        continue;
      }

      functions[key] = new lambda.Function(this, definition.id, {
        functionName: `${definition.binaryName}-${props.config.stage}`,
        runtime: lambda.Runtime.PROVIDED_AL2023,
        architecture: lambda.Architecture.X86_64,
        handler: "lib.handler",
        code: lambda.Code.fromBucket(
          props.artifactBucket,
          `${definition.binaryName}-${props.config.stage}-${props.parameters.commitSha}.zip`,
        ),
        memorySize: definition.memorySize,
        timeout: cdk.Duration.seconds(definition.timeoutSeconds),
        ephemeralStorageSize: cdk.Size.mebibytes(512),
        environment: lambdaEnvironment(definition, environmentContext),
      });
    }

    this.functions = functions as LambdaFunctions;
    grantRuntimeAccess(props, this.functions);
  }
}

function lambdaEnvironment(definition: LambdaDefinition, context: LambdaEnvironmentContext): Record<string, string> {
  const env = definition.environment?.(context) ?? baseEnvironment(context);
  return definition.postgres ? withPostgresEnvironment(context, env) : env;
}

function baseEnvironment(context: LambdaEnvironmentContext): Record<string, string> {
  return {
    DYNAMODB_TABLE_NAME: context.table.tableName,
  };
}

function withPostgresEnvironment(context: LambdaEnvironmentContext, env: Record<string, string>): Record<string, string> {
  return {
    ...env,
    POSTGRES_DATABASE: context.postgres.database,
    POSTGRES_HOST: context.postgres.host,
    POSTGRES_MAX_CONNECTIONS: context.postgres.maxConnections,
    POSTGRES_PASSWORD: context.postgres.password,
    POSTGRES_PORT: context.postgres.port,
    POSTGRES_USERNAME: context.postgres.username,
  };
}

function stageEnvironment(context: LambdaEnvironmentContext): Record<string, string> {
  return {
    ...baseEnvironment(context),
    STAGE: context.config.stage,
  };
}

function mailTemplateEnvironment(context: LambdaEnvironmentContext): Record<string, string> {
  return {
    ...baseEnvironment(context),
    COMMIT_SHA: context.commitSha,
    S3_BUCKET_NAME_TEMPLATES: context.mailTemplateBucket.bucketName,
    STAGE_NAME: context.config.stage,
  };
}

function openSearchWorkerEnvironment(context: LambdaEnvironmentContext): Record<string, string> {
  return withLocalStackPort(
    context.config,
    withOpenSearchCredentials(context.config, {
      OPENSEARCH_ENDPOINT_URL: context.search.endpointUrl,
      STAGE: context.config.stage,
    }),
  );
}

function grantRuntimeAccess(props: LambdasProps, functions: LambdaFunctions): void {
  for (const [key, fn] of Object.entries(functions) as [LambdaKey, lambda.Function | undefined][]) {
    if (!fn || key === "cloudWatchLogRetention" || key === "fxRateSync") {
      continue;
    }

    props.table.grantReadWriteData(fn);
  }

  props.queues.productPartnerIngest.queue.grantSendMessages(functions.productApiPartner);
  props.queues.productPartnerIngest.queue.grantSendMessages(functions.webhookApi);

  const mailTemplateReaders = [
    functions.notificationSend,
    functions.partnerShopApplicationWorkflow,
    functions.productUpdateNotifyUser,
    functions.searchFilterPercolateProduct,
  ];
  for (const fn of mailTemplateReaders) {
    props.mailTemplateBucket.grantRead(fn);
    fn.addToRolePolicy(
      new iam.PolicyStatement({
        actions: ["ses:SendEmail", "ses:SendRawEmail"],
        resources: ["*"],
      }),
    );
  }

  const openSearchWriters: lambda.Function[] = [
    functions.productApi,
    functions.productApiPartner,
    functions.productMaterializeOpenSearch,
    functions.productDeleteProduct,
    functions.productPartnerIngest,
    functions.searchFilterApi,
    functions.searchFilterOpenSearchSync,
    functions.searchFilterPercolateProduct,
    functions.shopApi,
    functions.shopOpenSearchIndex,
    functions.shopify,
    functions.userApi,
    functions.userOpenSearchIndex,
    functions.webhookApi,
  ];
  for (const fn of openSearchWriters) {
    props.search.grantReadWrite(fn);
  }

  functions.cloudWatchLogRetention.addToRolePolicy(
    new iam.PolicyStatement({
      actions: ["logs:DescribeLogGroups", "logs:PutRetentionPolicy"],
      resources: ["*"],
    }),
  );
}

export function addUserPoolEnvironment(functions: LambdaFunctions, userPoolId: string, publicClientId: string): void {
  const userPoolEnvUsers = [
    functions.newsletterApi,
    functions.productApi,
    functions.productApiPartner,
    functions.shopApi,
    functions.webhookApi,
  ];

  for (const fn of userPoolEnvUsers) {
    fn.addEnvironment("USER_POOL_ID", userPoolId);
    fn.addEnvironment("USER_POOL_PUBLIC_CLIENT_ID", publicClientId);
  }

  functions.userApi.addEnvironment("COGNITO_USER_POOL_ID", userPoolId);
}

export function grantCognitoAdminAccess(functions: LambdaFunctions, userPoolArn: string): void {
  const cognitoUsers = [
    functions.newsletterApi,
    functions.oauthApi,
    functions.productApi,
    functions.productApiPartner,
    functions.shopApi,
    functions.userApi,
    functions.webhookApi,
  ];

  for (const fn of cognitoUsers) {
    fn.addToRolePolicy(
      new iam.PolicyStatement({
        actions: ["cognito-idp:*"],
        resources: [userPoolArn],
      }),
    );
  }
}

export function importLambdaCatalog(scope: Construct, id: string, config: StageConfig): LambdaCatalog {
  const catalog = {} as Partial<Record<LambdaKey, lambda.IFunction>>;
  const importScope = new Construct(scope, id);

  for (const [key, definition] of Object.entries(LAMBDA_DEFINITIONS) as [LambdaKey, LambdaDefinition][]) {
    if (config.isEphemeral && definition.skipEphemeral) {
      continue;
    }

    catalog[key] = lambda.Function.fromFunctionName(
      importScope,
      `${definition.id}Import`,
      lambdaFunctionName(key, config.stage),
    );
  }

  return catalog as LambdaCatalog;
}

export function lambdaFunctionName(key: LambdaKey, stage: StageName): string {
  return `${LAMBDA_DEFINITIONS[key].binaryName}-${stage}`;
}

function withOpenSearchCredentials(config: StageConfig, env: Record<string, string>): Record<string, string> {
  if (config.isEphemeral) {
    return env;
  }

  return {
    ...env,
    OPENSEARCH_USERNAME: ssmValue(`/opensearch/${config.stage}/username`),
    OPENSEARCH_PASSWORD: ssmValue(`/opensearch/${config.stage}/password`),
  };
}

function withLocalStackPort(config: StageConfig, env: Record<string, string>): Record<string, string> {
  if (!config.isEphemeral) {
    return env;
  }

  return {
    ...env,
    LOCALSTACK_MAPPED_PORT: config.localStackMappedPort,
  };
}

function secretOrTest(config: StageConfig, name: string, testValue: string): string {
  return config.isEphemeral ? testValue : ssmSecret(config, name);
}

function ssmSecret(config: StageConfig, name: string): string {
  return ssmValue(`/secrets/${config.stage}/${name}`);
}
