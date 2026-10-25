import * as cdk from "aws-cdk-lib";
import * as ec2 from "aws-cdk-lib/aws-ec2";
import * as iam from "aws-cdk-lib/aws-iam";
import * as lambda from "aws-cdk-lib/aws-lambda";
import * as logs from "aws-cdk-lib/aws-logs";
import * as path from "node:path";
import * as s3 from "aws-cdk-lib/aws-s3";
import type * as ses from "aws-cdk-lib/aws-ses";
import { Construct } from "constructs";
import type { StageConfig, StageName } from "../config";
import { ssmValue } from "../config";
import { releaseLambdaCode, lambdaArtifactParameters } from "../release-artifacts";
import type { ApplicationParameters } from "../parameters";
import {
  apiEnvironment,
  marketingConsentSyncEnvironment,
  mcpEnvironment,
  notificationDeliveryEnvironment,
  providerSignupEnvironment,
  type LambdaEnvironmentContext,
} from "./lambda-environment";

import type { Network } from "./network";
import type { QueueCatalog } from "./queues";
import type { Search } from "./opensearch";
import type { PostgresConnectionSettings } from "./storage";

interface LambdaDefinition {
  readonly id: string;
  readonly binaryName: string;
  readonly memorySize: number;
  readonly timeoutSeconds: number;
  readonly reservedConcurrency?: (config: StageConfig) => number;
  readonly postgres?: boolean;
  readonly maxPostgresConnections?: number;
  readonly environment?: (context: LambdaEnvironmentContext) => Record<string, string>;
}

function defineLambdaDefinitions<T extends Record<string, LambdaDefinition>>(definitions: T): T {
  return definitions;
}

const LAMBDA_DEFINITIONS = defineLambdaDefinitions({
  auraHistoriaApi: {
    id: "AuraHistoriaApiLambda",
    binaryName: "aura-historia-api",
    memorySize: 512,
    postgres: true,
    timeoutSeconds: 15,
    environment: apiEnvironment,
  },
  auraHistoriaMcp: {
    id: "AuraHistoriaMcpLambda",
    binaryName: "aura-historia-mcp",
    reservedConcurrency: (config) => config.mcp.reservedConcurrency,
    memorySize: 512,
    postgres: true,
    maxPostgresConnections: 1,
    timeoutSeconds: 28,
    environment: mcpEnvironment,
  },
  cloudWatchLogRetention: {
    id: "CloudWatchLogRetentionLambda",
    binaryName: "cloudwatch-log-retention-lambda",
    memorySize: 128,
    timeoutSeconds: 10,
    environment: consentLogRetentionEnvironment,
  },
  cdcRouter: {
    id: "CdcRouterLambda",
    binaryName: "cdc-router-lambda",
    memorySize: 256,
    timeoutSeconds: 30,
  },

  backendCleanup: {
    id: "BackendCleanupLambda",
    binaryName: "backend-cleanup-lambda",
    memorySize: 128,
    postgres: true,
    // The hourly run includes the due public catalogue overview refresh (480 s statement timeout).
    timeoutSeconds: 540,
    environment: () => ({
      EXPIRY_CLEANUP_BATCH_SIZE: "100",
    }),
  },

  postConfirmation: {
    id: "PrimaryUserPoolPostConfirmationLambda",
    binaryName: "cognito-post-confirmation",
    memorySize: 256,
    postgres: true,
    timeoutSeconds: 5,
  },
  preSignUp: {
    id: "PrimaryUserPoolPreSignUpLambda",
    binaryName: "cognito-pre-sign-up",
    memorySize: 256,
    postgres: true,
    maxPostgresConnections: 1,
    timeoutSeconds: 5,
    environment: providerSignupEnvironment,
  },
  shopify: {
    id: "ShopifyLambda",
    binaryName: "shopify-lambda",
    memorySize: 256,
    postgres: true,
    timeoutSeconds: 30,
    environment: (context) => ({ PRODUCT_LISTING_INGESTION_QUEUE_URL: context.queues.productListingIngestion.queue.queueUrl }),
  },
  stripe: {
    id: "StripeLambda",
    binaryName: "stripe-lambda",
    memorySize: 256,
    postgres: true,
    timeoutSeconds: 30,
    environment: (context) => ({
      STRIPE_API_KEY: ssmValue(`/stripe/${context.config.stage}/api-key`),
      STRIPE_PRO_PRODUCT_ID: context.config.stripeProProductId,
      STRIPE_ULTIMATE_PRODUCT_ID: context.config.stripeUltimateProductId,
    }),
  },

  productListingOpenSearch: {
    id: "ProductListingOpenSearchLambda",
    binaryName: "product-listing-opensearch-lambda",
    memorySize: 512,
    postgres: true,
    timeoutSeconds: 45,
    environment: (context) => ({
      STAGE: context.config.stage,
      OPENSEARCH_ENDPOINT_URL: context.search.endpointUrl,
      OPENSEARCH_USERNAME: ssmValue(`/opensearch/${context.config.stage}/product-projector/username`),
      OPENSEARCH_PASSWORD: ssmValue(`/opensearch/${context.config.stage}/product-projector/password`),
    }),
  },
  productListingIngestion: {
    id: "ProductListingIngestionLambda",
    binaryName: "product-listing-ingestion-lambda",
    reservedConcurrency: () => 2,
    memorySize: 512,
    postgres: true,
    timeoutSeconds: 45,
  },
  productListingNormalization: {
    id: "ProductListingNormalizationLambda",
    binaryName: "product-listing-normalization-lambda",
    memorySize: 512,
    postgres: true,
    timeoutSeconds: 45,
  },
  productContentAssessment: {
    id: "ProductContentAssessmentLambda",
    binaryName: "product-content-assessment-lambda",
    memorySize: 512,
    postgres: true,
    timeoutSeconds: 45,
  },
  productEmbedding: {
    id: "ProductEmbeddingLambda",
    binaryName: "product-embedding-lambda",
    memorySize: 1024,
    postgres: true,
    timeoutSeconds: 60,
    environment: (context) => ({
      VERTEX_AI_PROJECT_ID: ssmValue(`/vertex-ai/${context.config.stage}/project-id`),
      VERTEX_AI_LOCATION: ssmValue(`/vertex-ai/${context.config.stage}/location`),
      AURA_HISTORIA_GOOGLE_ADC_CREDENTIALS_JSON: ssmValue(`/secrets/${context.config.stage}/google-application-credentials`),
    }),
  },
  productTranslation: {
    id: "ProductTranslationLambda",
    binaryName: "product-translation-lambda",
    memorySize: 512,
    postgres: true,
    timeoutSeconds: 45,
    environment: (context) => ({
      ANTHROPIC_API_KEY: ssmValue(`/anthropic/${context.config.stage}/api-key`),
    }),
  },
  notificationDelivery: {
    id: "NotificationDeliveryLambda",
    binaryName: "notification-delivery-lambda",
    memorySize: 512,
    postgres: true,
    timeoutSeconds: 45,
    environment: notificationDeliveryEnvironment,
  },
  marketingConsentSync: {
    id: "MarketingConsentSyncLambda",
    binaryName: "marketing-consent-sync-lambda",
    memorySize: 512,
    postgres: true,
    maxPostgresConnections: 1,
    timeoutSeconds: 45,
    environment: marketingConsentSyncEnvironment,
  },
  userIdentityDeletion: {
    id: "UserIdentityDeletionLambda",
    binaryName: "user-identity-deletion-lambda",
    memorySize: 256,
    postgres: true,
    maxPostgresConnections: 1,
    timeoutSeconds: 45,
  },
  listingGeography: {
    id: "ListingGeographyLambda",
    binaryName: "listing-geography-lambda",
    reservedConcurrency: () => 2,
    memorySize: 256,
    postgres: true,
    maxPostgresConnections: 1,
    timeoutSeconds: 45,
  },
  listingShipping: {
    id: "ListingShippingLambda",
    binaryName: "listing-shipping-lambda",
    reservedConcurrency: () => 2,
    memorySize: 256,
    postgres: true,
    maxPostgresConnections: 1,
    timeoutSeconds: 45,
  },
  geoResolution: {
    id: "GeoResolutionLambda",
    binaryName: "geo-resolution-lambda",
    reservedConcurrency: () => 2,
    memorySize: 256,
    postgres: true,
    maxPostgresConnections: 1,
    timeoutSeconds: 45,
  },
  searchFilterMatchNotification: {
    id: "SearchFilterMatchNotificationLambda",
    binaryName: "search-filter-match-notification-lambda",
    memorySize: 512,
    postgres: true,
    timeoutSeconds: 45,
  },
  watchlistNotification: {
    id: "WatchlistNotificationLambda",
    binaryName: "watchlist-notification-lambda",
    memorySize: 512,
    postgres: true,
    timeoutSeconds: 45,
  },
} as const);

export type LambdaKey = keyof typeof LAMBDA_DEFINITIONS;
export const API_LAMBDA_ALIAS_NAME = "live";
export function consentEvidenceLogGroupNames(stage: StageName): readonly [string, string] {
  return [
    `/aws/lambda/aura-historia-api-${stage}`,
    `/aws/lambda/cognito-post-confirmation-${stage}`,
  ];
}
export type LambdaCatalog = Record<LambdaKey, lambda.IFunction>;
export type LambdaFunctions = Record<LambdaKey, lambda.Function>;

export interface LambdasProps {
  readonly config: StageConfig;
  readonly emailConfigurationSet: ses.IConfigurationSet;
  readonly parameters: ApplicationParameters;
  readonly artifactBucket: s3.IBucket;
  readonly mailTemplateBucket: s3.IBucket;
  readonly postgres: PostgresConnectionSettings;
  readonly search: Search;
  readonly queues: QueueCatalog;
  readonly network?: Network;
}

export class Lambdas extends Construct {
  readonly functions: LambdaFunctions;
  readonly apiAlias: lambda.Alias;
  readonly mcpAlias: lambda.Alias;
  readonly postConfirmationAlias: lambda.Alias;
  readonly preSignUpAlias: lambda.Alias;
  readonly productListingOpenSearchVersion: lambda.Version;
  readonly productListingNormalizationVersion: lambda.Version;
  readonly productListingIngestionVersion: lambda.Version;
  readonly productContentAssessmentVersion: lambda.Version;
  readonly productEmbeddingVersion: lambda.Version;
  readonly productTranslationVersion: lambda.Version;
  readonly searchFilterMatchNotificationVersion: lambda.Version;
  readonly watchlistNotificationVersion: lambda.Version;
  readonly notificationDeliveryVersion: lambda.Version;
  readonly marketingConsentSyncVersion: lambda.Version;
  readonly userIdentityDeletionVersion: lambda.Version;
  readonly geoResolutionVersion: lambda.Version;
  readonly listingGeographyVersion: lambda.Version;
  readonly listingShippingVersion: lambda.Version;
  readonly backendCleanupVersion: lambda.Version | undefined;
  readonly cdcRouterVersion: lambda.Version | undefined;


  constructor(scope: Construct, id: string, props: LambdasProps) {
    super(scope, id);

    const postgresTlsRootCertificateLayer = new lambda.LayerVersion(this, "PostgresTlsRootCertificateLayer", {
      code: lambda.Code.fromAsset(path.join(__dirname, "../../assets/rds-ca-layer")),
      compatibleRuntimes: [lambda.Runtime.PROVIDED_AL2023],
      description: "Public AWS RDS root certificate bundle for PostgreSQL Lambdas",
    });
    const functions = {} as LambdaFunctions;
    const ingestionLogGroup = new logs.LogGroup(this, "ProductListingIngestionLogGroup", {
      logGroupName: `/aws/lambda/product-listing-ingestion-lambda-${props.config.stage}`,
      retention: logs.RetentionDays.ONE_MONTH,
      removalPolicy: cdk.RemovalPolicy.RETAIN,
    });
    const [apiEvidenceLogGroupName, postConfirmationEvidenceLogGroupName] =
      consentEvidenceLogGroupNames(props.config.stage);
    const consentEvidenceLogGroupRemovalPolicy = cdk.RemovalPolicy.RETAIN;
    const apiEvidenceLogGroup = new logs.LogGroup(this, "AuraHistoriaApiEvidenceLogGroup", {
      logGroupName: apiEvidenceLogGroupName,
      retention: logs.RetentionDays.INFINITE,
      removalPolicy: consentEvidenceLogGroupRemovalPolicy,
    });
    const postConfirmationEvidenceLogGroup = new logs.LogGroup(
      this,
      "PostConfirmationEvidenceLogGroup",
      {
        logGroupName: postConfirmationEvidenceLogGroupName,
        retention: logs.RetentionDays.INFINITE,
        removalPolicy: consentEvidenceLogGroupRemovalPolicy,
      },
    );
    const mcpLogGroup = new logs.LogGroup(this, "AuraHistoriaMcpLogGroup", {
      logGroupName: `/aws/lambda/aura-historia-mcp-${props.config.stage}`,
      retention: logs.RetentionDays.ONE_MONTH,
      removalPolicy: props.config.removalPolicy,
    });
    const logGroups: Partial<Record<LambdaKey, logs.ILogGroup>> = {
      productListingIngestion: ingestionLogGroup,
      auraHistoriaApi: apiEvidenceLogGroup,
      auraHistoriaMcp: mcpLogGroup,
      postConfirmation: postConfirmationEvidenceLogGroup,
    };
    const environmentContext: LambdaEnvironmentContext = {
      config: props.config,
      emailConfigurationSet: props.emailConfigurationSet,
      commitSha: props.parameters.commitSha,
      mailTemplatePrefix: props.parameters.mailTemplatePrefix,
      postgres: props.postgres,
      search: props.search,
      queues: props.queues,
    };

    for (const [key, definition] of Object.entries(LAMBDA_DEFINITIONS) as [LambdaKey, LambdaDefinition][]) {
      const networkProps = definition.postgres && props.network
        ? {
            vpc: props.network.vpc,
            vpcSubnets: { subnetType: ec2.SubnetType.PRIVATE_WITH_EGRESS },
            securityGroups: [props.network.applicationSecurityGroup],
          }
        : {};

      functions[key] = new lambda.Function(this, definition.id, {
        ...networkProps,
        functionName: `${definition.binaryName}-${props.config.stage}`,
        runtime: lambda.Runtime.PROVIDED_AL2023,
        architecture: lambda.Architecture.X86_64,
        handler: "lib.handler",
        code: releaseLambdaCode(cdk.Stack.of(this), props.artifactBucket, definition.binaryName),
        currentVersionOptions: { description: lambdaArtifactParameters(cdk.Stack.of(this), definition.binaryName).sha256 },
        memorySize: definition.memorySize,
        timeout: cdk.Duration.seconds(definition.timeoutSeconds),
        reservedConcurrentExecutions: definition.reservedConcurrency?.(props.config),
        logGroup: logGroups[key],
        ephemeralStorageSize: cdk.Size.mebibytes(512),
        environment: lambdaEnvironment(definition, environmentContext),
        layers: definition.postgres && postgresTlsRootCertificateLayer
          ? [postgresTlsRootCertificateLayer]
          : undefined,
      });
    }

    this.functions = functions as LambdaFunctions;
    this.apiAlias = new lambda.Alias(this, "AuraHistoriaApiAlias", {
      aliasName: API_LAMBDA_ALIAS_NAME,
      version: this.functions.auraHistoriaApi.currentVersion,
      description: "Stable HTTP API integration target",
    });
    this.mcpAlias = new lambda.Alias(this, "AuraHistoriaMcpAlias", {
      aliasName: API_LAMBDA_ALIAS_NAME,
      version: this.functions.auraHistoriaMcp.currentVersion,
      description: "Stable MCP HTTP integration target",
    });
    this.postConfirmationAlias = new lambda.Alias(this, "PostConfirmationAlias", {
      aliasName: API_LAMBDA_ALIAS_NAME,
      version: this.functions.postConfirmation.currentVersion,
    });
    this.preSignUpAlias = new lambda.Alias(this, "PreSignUpAlias", {
      aliasName: API_LAMBDA_ALIAS_NAME,
      version: this.functions.preSignUp.currentVersion,
    });
    this.productListingOpenSearchVersion = new lambda.Version(this, "ProductListingOpenSearchVersion", {
      lambda: this.functions.productListingOpenSearch,
      description: lambdaArtifactParameters(cdk.Stack.of(this), "product-listing-opensearch-lambda").sha256,
    });
    this.productListingNormalizationVersion = new lambda.Version(this, "ProductListingNormalizationVersion", {
      lambda: this.functions.productListingNormalization,
      description: lambdaArtifactParameters(cdk.Stack.of(this), "product-listing-normalization-lambda").sha256,
    });
    this.productListingIngestionVersion = new lambda.Version(this, "ProductListingIngestionVersion", {
      lambda: this.functions.productListingIngestion,
      description: lambdaArtifactParameters(cdk.Stack.of(this), "product-listing-ingestion-lambda").sha256,
    });
    this.productContentAssessmentVersion = new lambda.Version(this, "ProductContentAssessmentVersion", {
      lambda: this.functions.productContentAssessment,
      description: lambdaArtifactParameters(cdk.Stack.of(this), "product-content-assessment-lambda").sha256,
    });
    this.productEmbeddingVersion = new lambda.Version(this, "ProductEmbeddingVersion", {
      lambda: this.functions.productEmbedding,
      description: lambdaArtifactParameters(cdk.Stack.of(this), "product-embedding-lambda").sha256,
    });
    this.productTranslationVersion = new lambda.Version(this, "ProductTranslationVersion", {
      lambda: this.functions.productTranslation,
      description: lambdaArtifactParameters(cdk.Stack.of(this), "product-translation-lambda").sha256,
    });
    this.searchFilterMatchNotificationVersion = new lambda.Version(this, "SearchFilterMatchNotificationVersion", {
      lambda: this.functions.searchFilterMatchNotification,
      description: lambdaArtifactParameters(cdk.Stack.of(this), "search-filter-match-notification-lambda").sha256,
    });
    this.watchlistNotificationVersion = new lambda.Version(this, "WatchlistNotificationVersion", {
      lambda: this.functions.watchlistNotification,
      description: lambdaArtifactParameters(cdk.Stack.of(this), "watchlist-notification-lambda").sha256,
    });
    this.notificationDeliveryVersion = new lambda.Version(this, "NotificationDeliveryVersion", {
      lambda: this.functions.notificationDelivery,
      description: lambdaArtifactParameters(cdk.Stack.of(this), "notification-delivery-lambda").sha256,
    });
    this.userIdentityDeletionVersion = new lambda.Version(
      this,
      "UserIdentityDeletionVersion",
      {
        lambda: this.functions.userIdentityDeletion,
        description: lambdaArtifactParameters(cdk.Stack.of(this), "user-identity-deletion-lambda").sha256,
      },
    );
    this.marketingConsentSyncVersion = new lambda.Version(this, "MarketingConsentSyncVersion", {
      lambda: this.functions.marketingConsentSync,
      description: lambdaArtifactParameters(cdk.Stack.of(this), "marketing-consent-sync-lambda").sha256,
    });
    this.listingGeographyVersion = new lambda.Version(this, "ListingGeographyVersion", {
      lambda: this.functions.listingGeography,
      description: lambdaArtifactParameters(cdk.Stack.of(this), "listing-geography-lambda").sha256,
    });
    this.listingShippingVersion = new lambda.Version(this, "ListingShippingVersion", {
      lambda: this.functions.listingShipping,
      description: lambdaArtifactParameters(cdk.Stack.of(this), "listing-shipping-lambda").sha256,
    });
    this.geoResolutionVersion = new lambda.Version(this, "GeoResolutionVersion", {
      lambda: this.functions.geoResolution,
      description: lambdaArtifactParameters(cdk.Stack.of(this), "geo-resolution-lambda").sha256,
    });
    this.backendCleanupVersion = new lambda.Version(this, "BackendCleanupVersion", {
      lambda: this.functions.backendCleanup,
      description: lambdaArtifactParameters(cdk.Stack.of(this), "backend-cleanup-lambda").sha256,
    });
    this.cdcRouterVersion = new lambda.Version(this, "CdcRouterVersion", {
      lambda: this.functions.cdcRouter,
      description: lambdaArtifactParameters(cdk.Stack.of(this), "cdc-router-lambda").sha256,
    });
    grantRuntimeAccess(props, this.functions);
  }
}

function lambdaEnvironment(definition: LambdaDefinition, context: LambdaEnvironmentContext): Record<string, string> {
  const env = definition.environment?.(context) ?? {};
  if (!definition.postgres) {
    return env;
  }
  const postgresEnvironment = withPostgresEnvironment(context, env);
  return definition.maxPostgresConnections === undefined
    ? postgresEnvironment
    : { ...postgresEnvironment, POSTGRES_MAX_CONNECTIONS: String(definition.maxPostgresConnections) };
}

export function withPostgresEnvironment(
  context: Pick<LambdaEnvironmentContext, "postgres">,
  env: Record<string, string>,
): Record<string, string> {
  const connection = {
    ...env,
    POSTGRES_DATABASE: context.postgres.database,
    POSTGRES_HOST: context.postgres.host,
    POSTGRES_MAX_CONNECTIONS: context.postgres.maxConnections,
    POSTGRES_PORT: context.postgres.port,
    POSTGRES_TLS_ROOT_CERT: context.postgres.tlsRootCert,
  };
  if (context.postgres.secretArn) {
    return {
      ...connection,
      POSTGRES_SECRET_ARN: context.postgres.secretArn,
    };
  }
  throw new Error("PostgreSQL Lambda environment requires a runtime secret ARN.");
}

function grantRuntimeAccess(props: LambdasProps, functions: LambdaFunctions): void {
  functions.geoResolution.addToRolePolicy(new iam.PolicyStatement({
    actions: ["geo-places:Geocode"],
    resources: ["*"],
    conditions: { StringEquals: { "aws:RequestedRegion": "eu-central-1" } },
  }));
  const logGroupArnPattern = cdk.Stack.of(functions.cloudWatchLogRetention).formatArn({
    service: "logs",
    resource: "log-group",
    resourceName: "*",
    arnFormat: cdk.ArnFormat.COLON_RESOURCE_NAME,
  });
  functions.cloudWatchLogRetention.addToRolePolicy(
    new iam.PolicyStatement({
      actions: ["logs:DescribeLogGroups"],
      resources: ["*"],
    }),
  );
  functions.cloudWatchLogRetention.addToRolePolicy(
    new iam.PolicyStatement({
      actions: ["logs:PutRetentionPolicy"],
      resources: [logGroupArnPattern],
    }),
  );
  for (const producer of [functions.auraHistoriaApi, functions.shopify]) {
    producer.addToRolePolicy(new iam.PolicyStatement({
      actions: ["sqs:SendMessage"],
      resources: [props.queues.productListingIngestion.queue.queueArn],
    }));
  }
  props.search.grantIndexDocumentWrite(functions.productListingOpenSearch, "product-listings");
  functions.notificationDelivery.addToRolePolicy(new iam.PolicyStatement({
    actions: ["s3:GetObject"],
    resources: [props.mailTemplateBucket.arnForObjects("mail/*")],
  }));
  // Both senders explicitly select the managed stage-local set, not the shared identity's default.
  const sesSendResources = [
    cdk.Stack.of(props.mailTemplateBucket).formatArn({
      service: "ses",
      resource: "identity",
      resourceName: props.config.notificationEmail.identityDomain,
    }),
    cdk.Stack.of(props.mailTemplateBucket).formatArn({
      service: "ses",
      resource: "configuration-set",
      resourceName: props.emailConfigurationSet.configurationSetName,
    }),
  ];
  functions.notificationDelivery.addToRolePolicy(new iam.PolicyStatement({
    actions: ["ses:SendEmail"],
    resources: sesSendResources,
  }));
  functions.auraHistoriaApi.addToRolePolicy(new iam.PolicyStatement({
    actions: ["s3:GetObject"],
    resources: [props.mailTemplateBucket.arnForObjects(
      "mail/*/mjml/newsletter/confirmation/*",
    )],
  }));
  functions.auraHistoriaApi.addToRolePolicy(new iam.PolicyStatement({
    actions: ["ses:SendEmail"],
    resources: sesSendResources,
  }));

  if (props.postgres.secretArn) {
    for (const [key, definition] of Object.entries(LAMBDA_DEFINITIONS) as [LambdaKey, LambdaDefinition][]) {
      if (!definition.postgres) {
        continue;
      }
      functions[key]?.addToRolePolicy(
        new iam.PolicyStatement({
          actions: ["secretsmanager:GetSecretValue"],
          resources: [props.postgres.secretArn],
        }),
      );
    }
  }
}

export function addUserPoolEnvironment(
  functions: LambdaFunctions,
  userPoolId: string,
  publicClientId: string,
): void {
  const functionRegion = cdk.Stack.of(functions.auraHistoriaApi).region;
  const issuer = `https://cognito-idp.${functionRegion}.amazonaws.com/${userPoolId}`;

  functions.auraHistoriaApi.addEnvironment("AURA_HISTORIA_COGNITO_ISSUER", issuer);
  functions.auraHistoriaApi.addEnvironment("AURA_HISTORIA_COGNITO_JWKS_URL", `${issuer}/.well-known/jwks.json`);
  functions.auraHistoriaApi.addEnvironment("AURA_HISTORIA_COGNITO_APP_CLIENT_IDS", publicClientId);
  functions.auraHistoriaApi.addEnvironment("AURA_HISTORIA_COGNITO_USER_POOL_ID", userPoolId);
  functions.userIdentityDeletion.addEnvironment("AURA_HISTORIA_COGNITO_USER_POOL_ID", userPoolId);
}

export function grantCognitoAdminAccess(functions: LambdaFunctions, userPoolArn: string): void {
  functions.userIdentityDeletion.addToRolePolicy(
    new iam.PolicyStatement({
      actions: ["cognito-idp:ListUsers", "cognito-idp:AdminGetUser", "cognito-idp:AdminDeleteUser"],
      resources: [userPoolArn],
    }),
  );
  functions.auraHistoriaApi.addToRolePolicy(
    new iam.PolicyStatement({
      actions: ["cognito-idp:AdminUserGlobalSignOut", "cognito-idp:ListUsers"],
      resources: [userPoolArn],
    }),
  );
}

export function grantCognitoFederatedLinkingAccess(functions: LambdaFunctions): void {
  const preSignUp = functions.preSignUp;
  if (!preSignUp) return;

  const sameRegionUserPoolArn = cdk.Stack.of(preSignUp).formatArn({
    service: "cognito-idp",
    resource: "userpool",
    resourceName: "*",
    arnFormat: cdk.ArnFormat.SLASH_RESOURCE_NAME,
  });
  preSignUp.addToRolePolicy(new iam.PolicyStatement({
    actions: ["cognito-idp:ListUsers", "cognito-idp:AdminLinkProviderForUser"],
    resources: [sameRegionUserPoolArn],
  }));
}

function consentLogRetentionEnvironment(context: LambdaEnvironmentContext): Record<string, string> {
  return {
    CONSENT_EVIDENCE_LOG_GROUPS: JSON.stringify(consentEvidenceLogGroupNames(context.config.stage)),
  };
}

export function importLambdaCatalog(scope: Construct, id: string, config: StageConfig): LambdaCatalog {
  const catalog = {} as Partial<Record<LambdaKey, lambda.IFunction>>;
  const importScope = new Construct(scope, id);

  for (const [key, definition] of Object.entries(LAMBDA_DEFINITIONS) as [LambdaKey, LambdaDefinition][]) {
    if (key === "auraHistoriaApi" || key === "auraHistoriaMcp") {
      catalog[key] = lambda.Function.fromFunctionAttributes(
        importScope,
        `${definition.id}AliasImport`,
        {
          functionArn: cdk.Stack.of(scope).formatArn({
            service: "lambda",
            resource: "function",
            resourceName: `${lambdaFunctionName(key, config.stage)}:${API_LAMBDA_ALIAS_NAME}`,
            arnFormat: cdk.ArnFormat.COLON_RESOURCE_NAME,
          }),
          sameEnvironment: true,
        },
      );
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
