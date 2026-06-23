import * as cdk from "aws-cdk-lib";
import * as dynamodb from "aws-cdk-lib/aws-dynamodb";
import * as iam from "aws-cdk-lib/aws-iam";
import * as lambda from "aws-cdk-lib/aws-lambda";
import * as s3 from "aws-cdk-lib/aws-s3";
import { Construct } from "constructs";
import type { StageConfig } from "../config";
import type { ApplicationParameters } from "../parameters";
import type { Search } from "./opensearch";
import type { QueueCatalog } from "./queues";

interface LambdaDefinition {
  readonly id: string;
  readonly binaryName: string;
  readonly memorySize: number;
  readonly timeoutSeconds: number;
  readonly skipEphemeral?: boolean;
}

const LAMBDA_DEFINITIONS = {
  cloudWatchLogRetention: {
    id: "CloudWatchLogRetentionLambda",
    binaryName: "cloudwatch-log-retention-lambda",
    memorySize: 128,
    timeoutSeconds: 10,
  },
  newsletterApi: {
    id: "NewsletterApiLambda",
    binaryName: "newsletter-api",
    memorySize: 256,
    timeoutSeconds: 10,
  },
  notificationApi: {
    id: "NotificationApiLambda",
    binaryName: "notification-api",
    memorySize: 256,
    timeoutSeconds: 10,
  },
  notificationSend: {
    id: "NotificationSendLambda",
    binaryName: "notification-send",
    memorySize: 512,
    timeoutSeconds: 60,
  },
  oauthApi: {
    id: "OAuthApiLambda",
    binaryName: "oauth-api",
    memorySize: 128,
    timeoutSeconds: 10,
  },
  partnerShopApplicationApi: {
    id: "PartnerShopApplicationApiLambda",
    binaryName: "partner-shop-application-api",
    memorySize: 256,
    timeoutSeconds: 10,
  },
  partnerShopApplicationWorkflow: {
    id: "PartnerShopApplicationStepFunctionLambda",
    binaryName: "partner-shop-application-lambda",
    memorySize: 256,
    timeoutSeconds: 30,
  },
  postConfirmation: {
    id: "PrimaryUserPoolPostConfirmationLambda",
    binaryName: "cognito-post-confirmation",
    memorySize: 256,
    timeoutSeconds: 5,
  },
  productApi: {
    id: "ProductApiLambda",
    binaryName: "product-api",
    memorySize: 512,
    timeoutSeconds: 10,
  },
  productApiPartner: {
    id: "ProductApiPartnerLambda",
    binaryName: "product-api-partner",
    memorySize: 512,
    timeoutSeconds: 10,
  },
  productMaterializeOpenSearch: {
    id: "ProductMaterializeOpenSearchLambda",
    binaryName: "product-lambda-materialize-opensearch",
    memorySize: 512,
    timeoutSeconds: 60,
  },
  productPartnerIngest: {
    id: "ProductPartnerIngestLambda",
    binaryName: "product-lambda-ingest-partner-products",
    memorySize: 512,
    timeoutSeconds: 30,
  },
  productPipelineEmbedText: {
    id: "ProductPipelineEmbedTextLambda",
    binaryName: "product-pipeline-embed-text",
    memorySize: 512,
    timeoutSeconds: 60,
  },
  productPipelineTranslate: {
    id: "ProductPipelineTranslateLambda",
    binaryName: "product-pipeline-translate",
    memorySize: 512,
    timeoutSeconds: 60,
  },
  productUpdateNotifyUser: {
    id: "ProductUpdateNotifyUserLambda",
    binaryName: "product-lambda-update-notify-user",
    memorySize: 512,
    timeoutSeconds: 60,
  },
  productWatchlistApi: {
    id: "ProductWatchlistApiLambda",
    binaryName: "product-watchlist-api",
    memorySize: 256,
    timeoutSeconds: 10,
  },
  searchFilterApi: {
    id: "SearchFilterApiLambda",
    binaryName: "search-filter-api",
    memorySize: 512,
    timeoutSeconds: 10,
  },
  searchFilterOpenSearchSync: {
    id: "SearchFilterOpenSearchSyncLambda",
    binaryName: "search-filter-lambda-opensearch-sync",
    memorySize: 256,
    timeoutSeconds: 30,
  },
  searchFilterPercolateProduct: {
    id: "SearchFilterPercolateProductLambda",
    binaryName: "search-filter-lambda-percolate-product",
    memorySize: 512,
    timeoutSeconds: 60,
  },
  shopApi: {
    id: "ShopApiLambda",
    binaryName: "shop-api",
    memorySize: 512,
    timeoutSeconds: 10,
  },
  shopOpenSearchIndex: {
    id: "ShopOpenSearchIndexLambda",
    binaryName: "shop-lambda-opensearch-index",
    memorySize: 256,
    timeoutSeconds: 30,
  },
  shopify: {
    id: "ShopifyLambda",
    binaryName: "shopify-lambda",
    memorySize: 256,
    timeoutSeconds: 30,
  },
  stripeApi: {
    id: "StripeApiLambda",
    binaryName: "stripe-api",
    memorySize: 256,
    timeoutSeconds: 10,
  },
  stripe: {
    id: "StripeLambda",
    binaryName: "stripe-lambda",
    memorySize: 256,
    timeoutSeconds: 30,
  },
  userApi: {
    id: "UserApiLambda",
    binaryName: "user-api",
    memorySize: 256,
    timeoutSeconds: 10,
  },
  userOpenSearchIndex: {
    id: "UserOpenSearchIndexLambda",
    binaryName: "user-lambda-index-opensearch",
    memorySize: 256,
    timeoutSeconds: 30,
  },
  userTierUpdate: {
    id: "UserTierUpdateLambda",
    binaryName: "user-lambda-tier-update",
    memorySize: 256,
    timeoutSeconds: 30,
  },
  webhookApi: {
    id: "WebhookApiLambda",
    binaryName: "webhook-api",
    memorySize: 512,
    timeoutSeconds: 10,
  },
  fxRateSync: {
    id: "FxRateSyncLambda",
    binaryName: "fxrate-lambda",
    memorySize: 128,
    timeoutSeconds: 10,
    skipEphemeral: true,
  },
} as const satisfies Record<string, LambdaDefinition>;

export type LambdaKey = keyof typeof LAMBDA_DEFINITIONS;
export type LambdaCatalog = Partial<Record<LambdaKey, lambda.Function>> &
  Record<Exclude<LambdaKey, "fxRateSync">, lambda.Function>;

export interface LambdasProps {
  readonly config: StageConfig;
  readonly parameters: ApplicationParameters;
  readonly artifactBucket: s3.IBucket;
  readonly mailTemplateBucket: s3.IBucket;
  readonly table: dynamodb.Table;
  readonly queues: QueueCatalog;
  readonly search: Search;
}

export class Lambdas extends Construct {
  readonly functions: LambdaCatalog;

  constructor(scope: Construct, id: string, props: LambdasProps) {
    super(scope, id);

    const functions = {} as Partial<Record<LambdaKey, lambda.Function>>;

    for (const [key, definition] of Object.entries(LAMBDA_DEFINITIONS) as [LambdaKey, LambdaDefinition][]) {
      if (props.config.isEphemeral && definition.skipEphemeral) {
        continue;
      }

      functions[key] = new lambda.Function(this, definition.id, {
        functionName: `${definition.binaryName}-${props.parameters.stageName}`,
        runtime: lambda.Runtime.PROVIDED_AL2023,
        architecture: lambda.Architecture.X86_64,
        handler: "lib.handler",
        code: lambda.Code.fromBucket(
          props.artifactBucket,
          `${definition.binaryName}-${props.parameters.stageName}-${props.parameters.commitSha}.zip`,
        ),
        memorySize: definition.memorySize,
        timeout: cdk.Duration.seconds(definition.timeoutSeconds),
        ephemeralStorageSize: cdk.Size.mebibytes(512),
        environment: environmentFor(key, props),
      });
    }

    this.functions = functions as LambdaCatalog;
    grantRuntimeAccess(props, this.functions);
  }
}

function environmentFor(key: LambdaKey, props: LambdasProps): Record<string, string> {
  const base = {
    DYNAMODB_TABLE_NAME: props.table.tableName,
  };
  const stage = props.parameters.stage;
  const stageName = props.parameters.stageName;
  const commitSha = props.parameters.commitSha;
  const opensearchEndpoint = props.search.endpointUrl;
  const localStackPort = props.parameters.localStackMappedPort;
  const mailBucket = props.mailTemplateBucket.bucketName;

  switch (key) {
    case "cloudWatchLogRetention":
      return {};
    case "newsletterApi":
      return {
        ...base,
        STAGE: stage,
        ZOHO_ACCOUNTS_URL: secretOrTest(props.config, "zoho-accounts-url", "https://accounts.zoho.eu"),
        ZOHO_CAMPAIGNS_URL: secretOrTest(props.config, "zoho-campaigns-url", "https://campaigns.zoho.eu"),
        ZOHO_CLIENT_ID: secretOrTest(props.config, "zoho-client-id", "test-zoho-client-id"),
        ZOHO_CLIENT_SECRET: secretOrTest(props.config, "zoho-client-secret", "test-zoho-client-secret"),
        ZOHO_LIST_KEY: secretOrTest(props.config, "zoho-list-key", "test-zoho-list-key"),
        ZOHO_REFRESH_TOKEN: secretOrTest(props.config, "zoho-refresh-token", "test-zoho-refresh-token"),
      };
    case "notificationApi":
    case "productWatchlistApi":
      return { ...base, STAGE: stage };
    case "notificationSend":
    case "productUpdateNotifyUser":
      return {
        ...base,
        COMMIT_SHA: commitSha,
        S3_BUCKET_NAME_TEMPLATES: mailBucket,
        STAGE_NAME: stageName,
      };
    case "oauthApi":
      return withLocalStackPort(props.config, { ...base, STAGE: stage }, localStackPort);
    case "partnerShopApplicationApi":
      return { ...base, STAGE: stage };
    case "partnerShopApplicationWorkflow":
      return {
        ...base,
        COMMIT_SHA: commitSha,
        GOOGLE_GEOCODING_API_KEY: secretOrTest(props.config, "google-geocoding-api-key", "test-key"),
        S3_BUCKET_NAME_TEMPLATES: mailBucket,
        STAGE_NAME: stageName,
      };
    case "postConfirmation":
      return base;
    case "productApi":
      return withLocalStackPort(
        props.config,
        withOpenSearchCredentials(props.config, {
          ...base,
          ...(props.config.isEphemeral
            ? { GEMINI_API_KEY: "test-key" }
            : { GOOGLE_APPLICATION_CREDENTIALS: ssmSecret(props.config, "google-application-credentials") }),
          OPENSEARCH_ENDPOINT_URL: opensearchEndpoint,
          STAGE: stage,
        }),
        localStackPort,
      );
    case "productApiPartner":
      return withLocalStackPort(
        props.config,
        withOpenSearchCredentials(props.config, {
          ...base,
          ASYNC_PRODUCT_COMMAND_QUEUE_URL: props.queues.productPartnerIngest.queue.queueUrl,
          ...(props.config.isEphemeral
            ? {}
            : {
                GEMINI_API_KEY: ssmSecret(props.config, "gemini-api-key"),
                GOOGLE_GEOCODING_API_KEY: ssmSecret(props.config, "google-geocoding-api-key"),
              }),
          OPENSEARCH_ENDPOINT_URL: opensearchEndpoint,
          STAGE: stage,
        }),
        localStackPort,
      );
    case "productMaterializeOpenSearch":
      return withLocalStackPort(
        props.config,
        withOpenSearchCredentials(props.config, {
          ...base,
          OPENSEARCH_ENDPOINT_URL: opensearchEndpoint,
          STAGE: stage,
        }),
        localStackPort,
      );
    case "productPartnerIngest":
      return withLocalStackPort(
        props.config,
        withOpenSearchCredentials(props.config, {
          ...base,
          GEMINI_API_KEY: secretOrTest(props.config, "gemini-api-key", "test-key"),
          GOOGLE_GEOCODING_API_KEY: secretOrTest(props.config, "google-geocoding-api-key", "test-key"),
          OPENSEARCH_ENDPOINT_URL: opensearchEndpoint,
          STAGE: stage,
        }),
        localStackPort,
      );
    case "productPipelineEmbedText":
      return props.config.isEphemeral
        ? { ...base, GEMINI_API_KEY: "test-key" }
        : { ...base, GOOGLE_APPLICATION_CREDENTIALS: ssmSecret(props.config, "google-application-credentials") };
    case "productPipelineTranslate":
      return {
        ...base,
        GEMINI_API_KEY: secretOrTest(props.config, "gemini-api-key", "test-key"),
        STAGE: stage,
      };
    case "searchFilterApi":
      return withLocalStackPort(
        props.config,
        withOpenSearchCredentials(props.config, {
          ...base,
          GEMINI_API_KEY: secretOrTest(props.config, "gemini-api-key", "test-key"),
          ...(props.config.isEphemeral ? {} : { GOOGLE_APPLICATION_CREDENTIALS: ssmSecret(props.config, "google-application-credentials") }),
          OPENSEARCH_ENDPOINT_URL: opensearchEndpoint,
          STAGE: stage,
        }),
        localStackPort,
      );
    case "searchFilterOpenSearchSync":
    case "shopOpenSearchIndex":
    case "userOpenSearchIndex":
      return withLocalStackPort(
        props.config,
        withOpenSearchCredentials(props.config, {
          OPENSEARCH_ENDPOINT_URL: opensearchEndpoint,
          STAGE: stage,
        }),
        localStackPort,
      );
    case "searchFilterPercolateProduct":
      return withLocalStackPort(
        props.config,
        withOpenSearchCredentials(props.config, {
          ...base,
          COMMIT_SHA: commitSha,
          GEMINI_API_KEY: secretOrTest(props.config, "gemini-api-key", "test-key"),
          OPENSEARCH_ENDPOINT_URL: opensearchEndpoint,
          S3_BUCKET_NAME_TEMPLATES: mailBucket,
          STAGE_NAME: stageName,
        }),
        localStackPort,
      );
    case "shopApi":
      return withLocalStackPort(
        props.config,
        withOpenSearchCredentials(props.config, {
          ...base,
          GOOGLE_GEOCODING_API_KEY: secretOrTest(props.config, "google-geocoding-api-key", "test-key"),
          OPENSEARCH_ENDPOINT_URL: opensearchEndpoint,
          STAGE: stage,
        }),
        localStackPort,
      );
    case "shopify":
      return withOpenSearchCredentials(props.config, {
        ...base,
        GEMINI_API_KEY: secretOrTest(props.config, "gemini-api-key", "test-key"),
        GOOGLE_GEOCODING_API_KEY: secretOrTest(props.config, "google-geocoding-api-key", "test-key"),
        OPENSEARCH_ENDPOINT_URL: opensearchEndpoint,
      });
    case "stripeApi":
      return {
        ...base,
        STAGE: stage,
        ...(props.config.isEphemeral ? {} : { STRIPE_API_KEY: ssmSecret(props.config, "stripe-api-key") }),
        STRIPE_CHECKOUT_CANCEL_URL: props.config.stripeCheckoutCancelUrl,
        STRIPE_CHECKOUT_SUCCESS_URL: props.config.stripeCheckoutSuccessUrl,
        STRIPE_PORTAL_RETURN_URL: props.config.stripePortalReturnUrl,
        STRIPE_PRO_MONTHLY_PRICE_ID: props.parameters.stripeProMonthlyPriceId,
        STRIPE_PRO_YEARLY_PRICE_ID: props.parameters.stripeProYearlyPriceId,
        STRIPE_ULTIMATE_MONTHLY_PRICE_ID: props.parameters.stripeUltimateMonthlyPriceId,
        STRIPE_ULTIMATE_YEARLY_PRICE_ID: props.parameters.stripeUltimateYearlyPriceId,
      };
    case "stripe":
      return {
        ...base,
        STRIPE_PRO_PRODUCT_ID: props.parameters.stripeProProductId,
        STRIPE_ULTIMATE_PRODUCT_ID: props.parameters.stripeUltimateProductId,
      };
    case "userApi":
      return withLocalStackPort(
        props.config,
        withOpenSearchCredentials(props.config, {
          ...base,
          ...(props.config.isEphemeral ? {} : { GOOGLE_GEOCODING_API_KEY: ssmSecret(props.config, "google-geocoding-api-key") }),
          OPENSEARCH_ENDPOINT_URL: opensearchEndpoint,
          STAGE: stage,
        }),
        localStackPort,
      );
    case "userTierUpdate":
      return base;
    case "webhookApi":
      return withLocalStackPort(
        props.config,
        withOpenSearchCredentials(props.config, {
          ...base,
          ASYNC_PRODUCT_COMMAND_QUEUE_URL: props.queues.productPartnerIngest.queue.queueUrl,
          ...(props.config.isEphemeral
            ? {}
            : {
                GEMINI_API_KEY: ssmSecret(props.config, "gemini-api-key"),
                GOOGLE_GEOCODING_API_KEY: ssmSecret(props.config, "google-geocoding-api-key"),
              }),
          OPENSEARCH_ENDPOINT_URL: opensearchEndpoint,
          STAGE: stage,
        }),
        localStackPort,
      );
    case "fxRateSync":
      return {
        ...base,
        FXRATES_API_TOKEN: "{{resolve:ssm:/fxratesapi/prod/api-token}}",
      };
  }
}

function grantRuntimeAccess(props: LambdasProps, functions: LambdaCatalog): void {
  for (const [key, fn] of Object.entries(functions) as [LambdaKey, lambda.Function | undefined][]) {
    if (!fn || key === "cloudWatchLogRetention") {
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

export function addUserPoolEnvironment(
  functions: LambdaCatalog,
  userPoolId: string,
  publicClientId: string,
  adminClientId: string,
): void {
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
    fn.addEnvironment("USER_POOL_ADMIN_CLIENT_ID", adminClientId);
  }

  functions.userApi.addEnvironment("COGNITO_USER_POOL_ID", userPoolId);
}

export function grantCognitoAdminAccess(functions: LambdaCatalog, userPoolArn: string): void {
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

function withOpenSearchCredentials(config: StageConfig, env: Record<string, string>): Record<string, string> {
  if (config.isEphemeral) {
    return env;
  }

  return {
    ...env,
    OPENSEARCH_USERNAME: `{{resolve:ssm:/opensearch/${config.stage}/username}}`,
    OPENSEARCH_PASSWORD: `{{resolve:ssm:/opensearch/${config.stage}/password}}`,
  };
}

function withLocalStackPort(config: StageConfig, env: Record<string, string>, localStackPort: string): Record<string, string> {
  if (!config.isEphemeral) {
    return env;
  }

  return {
    ...env,
    LOCALSTACK_MAPPED_PORT: localStackPort,
  };
}

function secretOrTest(config: StageConfig, name: string, testValue: string): string {
  return config.isEphemeral ? testValue : ssmSecret(config, name);
}

function ssmSecret(config: StageConfig, name: string): string {
  return `{{resolve:ssm:/secrets/${config.stage}/${name}}}`;
}
