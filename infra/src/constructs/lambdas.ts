import * as cdk from "aws-cdk-lib";
import * as ec2 from "aws-cdk-lib/aws-ec2";
import * as iam from "aws-cdk-lib/aws-iam";
import * as lambda from "aws-cdk-lib/aws-lambda";
import * as logs from "aws-cdk-lib/aws-logs";
import * as path from "node:path";
import * as s3 from "aws-cdk-lib/aws-s3";
import { Construct } from "constructs";
import type { StageConfig, StageName } from "../config";
import { MAIL_TEMPLATE_BUCKET_NAME, ssmValue } from "../config";
import type { ApplicationParameters } from "../parameters";

import type { Network } from "./network";
import type { QueueCatalog } from "./queues";
import type { Search } from "./opensearch";
import type { PostgresConnectionSettings, PostgresMigrationConnectionSettings } from "./storage";

interface LambdaEnvironmentContext {
  readonly config: StageConfig;
  readonly commitSha: string;
  readonly searchFilterClassifierModel: string;
  readonly searchFilterMatchShouldShowThresholdBps: string;
  readonly postgres: PostgresConnectionSettings;
  readonly search: Search;
  readonly queues: QueueCatalog;
}

interface LambdaDefinition {
  readonly id: string;
  readonly binaryName: string;
  readonly memorySize: number;
  readonly timeoutSeconds: number;
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
    timeoutSeconds: 10,
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
      VERTEX_AI_PROJECT_ID: ssmValue(`/vertex-ai/${context.config.stage}/project-id`),
      VERTEX_AI_LOCATION: ssmValue(`/vertex-ai/${context.config.stage}/location`),
      VERTEX_AI_MODEL: ssmValue(`/vertex-ai/${context.config.stage}/model`),
      AURA_HISTORIA_GOOGLE_ADC_CREDENTIALS_JSON: ssmValue(`/secrets/${context.config.stage}/google-application-credentials`),
    }),
  },
  searchFilterProjection: {
    id: "SearchFilterProjectionLambda",
    binaryName: "search-filter-projection-lambda",
    memorySize: 512,
    postgres: true,
    timeoutSeconds: 45,
    environment: (context) => ({
      STAGE: context.config.stage,
      OPENSEARCH_ENDPOINT_URL: context.search.endpointUrl,
      OPENSEARCH_USERNAME: ssmValue(`/opensearch/${context.config.stage}/filter-projector/username`),
      OPENSEARCH_PASSWORD: ssmValue(`/opensearch/${context.config.stage}/filter-projector/password`),
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
  searchFilterPercolator: {
    id: "SearchFilterPercolatorLambda",
    binaryName: "search-filter-percolator-lambda",
    memorySize: 512,
    postgres: true,
    timeoutSeconds: 45,
    environment: (context) => ({
      STAGE: context.config.stage,
      OPENSEARCH_ENDPOINT_URL: context.search.endpointUrl,
      CLASSIFIER_MODEL_PROVIDER: context.config.searchFilterClassifier.provider,
      CLASSIFIER_MODEL: context.searchFilterClassifierModel,
      SEARCH_FILTER_MATCH_SHOULD_SHOW_THRESHOLD_BPS: context.searchFilterMatchShouldShowThresholdBps,
      CLOUDFLARE_ACCOUNT_ID: ssmValue(`/cloudflare/${context.config.stage}/account-id`),
      CLOUDFLARE_API_TOKEN_SSM_PARAMETER: `/secrets/${context.config.stage}/cloudflare-workers-ai-api-token`,
      OPENSEARCH_USERNAME: ssmValue(`/opensearch/${context.config.stage}/percolator/username`),
      OPENSEARCH_PASSWORD: ssmValue(`/opensearch/${context.config.stage}/percolator/password`),
    }),
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
  readonly productListingOpenSearchVersion: lambda.Version;
  readonly productListingNormalizationVersion: lambda.Version;
  readonly productListingIngestionVersion: lambda.Version;
  readonly productContentAssessmentVersion: lambda.Version;
  readonly productEmbeddingVersion: lambda.Version;
  readonly productTranslationVersion: lambda.Version;
  readonly searchFilterProjectionVersion: lambda.Version;
  readonly searchFilterPercolatorVersion: lambda.Version;
  readonly searchFilterMatchNotificationVersion: lambda.Version;
  readonly watchlistNotificationVersion: lambda.Version;
  readonly notificationDeliveryVersion: lambda.Version;
  readonly marketingConsentSyncVersion: lambda.Version;
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
    const environmentContext: LambdaEnvironmentContext = {
      config: props.config,
      commitSha: props.parameters.commitSha,
      searchFilterClassifierModel: props.parameters.searchFilterClassifierModel,
      searchFilterMatchShouldShowThresholdBps:
        props.parameters.searchFilterMatchShouldShowThresholdBps,
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
        code: lambda.Code.fromBucket(
          props.artifactBucket,
          `${definition.binaryName}-${props.config.stage}-${props.parameters.commitSha}.zip`,
        ),
        currentVersionOptions: key === "auraHistoriaApi"
          ? { description: `aura-historia-api-${props.parameters.commitSha}` }
          : undefined,
        memorySize: definition.memorySize,
        timeout: cdk.Duration.seconds(definition.timeoutSeconds),
        reservedConcurrentExecutions: key === "productListingIngestion" ? 2 : undefined,
        logGroup: key === "productListingIngestion"
          ? ingestionLogGroup
          : key === "auraHistoriaApi"
            ? apiEvidenceLogGroup
            : key === "postConfirmation"
              ? postConfirmationEvidenceLogGroup
              : undefined,
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
    this.productListingOpenSearchVersion = new lambda.Version(this, "ProductListingOpenSearchVersion", {
      lambda: this.functions.productListingOpenSearch,
      description: `product-listing-opensearch-${props.parameters.commitSha}`,
    });
    this.productListingNormalizationVersion = new lambda.Version(this, "ProductListingNormalizationVersion", {
      lambda: this.functions.productListingNormalization,
      description: `product-listing-normalization-${props.parameters.commitSha}`,
    });
    this.productListingIngestionVersion = new lambda.Version(this, "ProductListingIngestionVersion", {
      lambda: this.functions.productListingIngestion,
      description: `product-listing-ingestion-${props.parameters.commitSha}`,
    });
    this.productContentAssessmentVersion = new lambda.Version(this, "ProductContentAssessmentVersion", {
      lambda: this.functions.productContentAssessment,
      description: `product-content-assessment-${props.parameters.commitSha}`,
    });
    this.productEmbeddingVersion = new lambda.Version(this, "ProductEmbeddingVersion", {
      lambda: this.functions.productEmbedding,
      description: `product-embedding-${props.parameters.commitSha}`,
    });
    this.productTranslationVersion = new lambda.Version(this, "ProductTranslationVersion", {
      lambda: this.functions.productTranslation,
      description: `product-translation-${props.parameters.commitSha}`,
    });
    this.searchFilterProjectionVersion = new lambda.Version(this, "SearchFilterProjectionVersion", {
      lambda: this.functions.searchFilterProjection,
      description: `search-filter-projection-${props.parameters.commitSha}`,
    });
    this.searchFilterPercolatorVersion = new lambda.Version(this, "SearchFilterPercolatorVersion", {
      lambda: this.functions.searchFilterPercolator,
      description: `search-filter-percolator-${props.parameters.commitSha}`,
    });
    this.searchFilterMatchNotificationVersion = new lambda.Version(this, "SearchFilterMatchNotificationVersion", {
      lambda: this.functions.searchFilterMatchNotification,
      description: `search-filter-match-notification-${props.parameters.commitSha}`,
    });
    this.watchlistNotificationVersion = new lambda.Version(this, "WatchlistNotificationVersion", {
      lambda: this.functions.watchlistNotification,
      description: `watchlist-notification-${props.parameters.commitSha}`,
    });
    this.notificationDeliveryVersion = new lambda.Version(this, "NotificationDeliveryVersion", {
      lambda: this.functions.notificationDelivery,
      description: `notification-delivery-${props.parameters.commitSha}`,
    });
    this.marketingConsentSyncVersion = new lambda.Version(this, "MarketingConsentSyncVersion", {
      lambda: this.functions.marketingConsentSync,
      description: `marketing-consent-sync-${props.parameters.commitSha}`,
    });
    this.backendCleanupVersion = new lambda.Version(this, "BackendCleanupVersion", {
      lambda: this.functions.backendCleanup,
      description: `backend-cleanup-${props.parameters.commitSha}`,
    });
    this.cdcRouterVersion = new lambda.Version(this, "CdcRouterVersion", {
      lambda: this.functions.cdcRouter,
      description: `cdc-router-${props.parameters.commitSha}`,
    });
    grantRuntimeAccess(props, this.functions);
  }
}

export interface InitializationLambdasProps {
  readonly config: StageConfig;
  readonly commitSha: string;
  readonly artifactBucket: s3.IBucket;
  readonly migrationPostgres: PostgresMigrationConnectionSettings;
  readonly postgres: PostgresConnectionSettings;
  readonly network: Network;
}

/** Private migration runtime. It exists before normal compute and has no event source. */
export class InitializationLambdas extends Construct {
  readonly databaseMigration: lambda.Function;

  constructor(scope: Construct, id: string, props: InitializationLambdasProps) {
    super(scope, id);

    const postgresTlsRootCertificateLayer = new lambda.LayerVersion(this, "PostgresTlsRootCertificateLayer", {
      code: lambda.Code.fromAsset(path.join(__dirname, "../../assets/rds-ca-layer")),
      compatibleRuntimes: [lambda.Runtime.PROVIDED_AL2023],
      description: "Public AWS RDS root certificate bundle for PostgreSQL Lambdas",
    });
    this.databaseMigration = new lambda.Function(this, "DatabaseMigrationLambda", {
      vpc: props.network.vpc,
      vpcSubnets: { subnetType: ec2.SubnetType.PRIVATE_WITH_EGRESS },
      securityGroups: [props.network.migrationSecurityGroup],
      functionName: `database-migration-lambda-${props.config.stage}`,
      runtime: lambda.Runtime.PROVIDED_AL2023,
      architecture: lambda.Architecture.X86_64,
      handler: "lib.handler",
      code: lambda.Code.fromBucket(
        props.artifactBucket,
        `database-migration-lambda-${props.config.stage}-${props.commitSha}.zip`,
      ),
      memorySize: 512,
      timeout: cdk.Duration.seconds(840),
      ephemeralStorageSize: cdk.Size.mebibytes(512),
      environment: withMigrationPostgresEnvironment({ migrationPostgres: props.migrationPostgres }, {}),
      layers: [postgresTlsRootCertificateLayer],
    });
    this.databaseMigration.addToRolePolicy(
      new iam.PolicyStatement({
        actions: ["secretsmanager:GetSecretValue"],
        resources: [
          props.migrationPostgres.adminSecretArn,
          props.migrationPostgres.runtimeSecretArn,
          props.migrationPostgres.migrationSecretArn,
          props.migrationPostgres.replicationSecretArn,
        ],
      }),
    );

    const fxRateSync = new lambda.Function(this, "FxRateSyncLambda", {
      vpc: props.network.vpc,
      vpcSubnets: { subnetType: ec2.SubnetType.PRIVATE_WITH_EGRESS },
      securityGroups: [props.network.applicationSecurityGroup],
      functionName: `fxrate-lambda-${props.config.stage}`,
      runtime: lambda.Runtime.PROVIDED_AL2023,
      architecture: lambda.Architecture.X86_64,
      handler: "lib.handler",
      code: lambda.Code.fromBucket(
        props.artifactBucket,
        `fxrate-lambda-${props.config.stage}-${props.commitSha}.zip`,
      ),
      memorySize: 128,
      timeout: cdk.Duration.seconds(10),
      ephemeralStorageSize: cdk.Size.mebibytes(512),
      environment: withPostgresEnvironment({ postgres: props.postgres }, {
        FXRATES_API_TOKEN: ssmValue(`/fxratesapi/${props.config.stage}/api-token`),
      }),
      layers: [postgresTlsRootCertificateLayer],
    });
    if (!props.postgres.secretArn) {
      throw new Error("Initialization FX Lambda requires a PostgreSQL runtime secret.");
    }
    fxRateSync.addToRolePolicy(new iam.PolicyStatement({
      actions: ["secretsmanager:GetSecretValue"],
      resources: [props.postgres.secretArn],
    }));
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

function withPostgresEnvironment(
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

function withMigrationPostgresEnvironment(
  context: { readonly migrationPostgres: PostgresMigrationConnectionSettings },
  env: Record<string, string>,
): Record<string, string> {
  const postgres = context.migrationPostgres;
  if (!postgres) {
    throw new Error("Migration Lambda requires real PostgreSQL migration connection settings.");
  }
  return {
    ...env,
    POSTGRES_ADMIN_SECRET_ARN: postgres.adminSecretArn,
    POSTGRES_DATABASE: postgres.database,
    POSTGRES_HOST: postgres.host,
    POSTGRES_MAX_CONNECTIONS: postgres.maxConnections,
    POSTGRES_MIGRATION_SECRET_ARN: postgres.migrationSecretArn,
    POSTGRES_PORT: postgres.port,
    POSTGRES_REPLICATION_SECRET_ARN: postgres.replicationSecretArn,
    POSTGRES_RUNTIME_SECRET_ARN: postgres.runtimeSecretArn,
    POSTGRES_TLS_ROOT_CERT: postgres.tlsRootCert,
  };
}

function grantRuntimeAccess(props: LambdasProps, functions: LambdaFunctions): void {
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
  props.search.grantIndexDocumentWrite(functions.productListingOpenSearch);
  props.search.grantIndexDocumentWrite(functions.searchFilterProjection);
  props.search.grantRead(functions.searchFilterPercolator);
  functions.searchFilterPercolator.addToRolePolicy(new iam.PolicyStatement({
    actions: ["ssm:GetParameter"],
    resources: [cdk.Stack.of(functions.searchFilterPercolator).formatArn({
      service: "ssm",
      resource: "parameter",
      resourceName: `secrets/${props.config.stage}/cloudflare-workers-ai-api-token`,
      arnFormat: cdk.ArnFormat.SLASH_RESOURCE_NAME,
    })],
  }));
  functions.notificationDelivery.addToRolePolicy(new iam.PolicyStatement({
    actions: ["s3:GetObject"],
    resources: [props.mailTemplateBucket.arnForObjects(`${props.config.stage}/${props.parameters.commitSha}/*`)],
  }));
  functions.notificationDelivery.addToRolePolicy(new iam.PolicyStatement({
    actions: ["ses:SendEmail"],
    resources: [cdk.Stack.of(props.mailTemplateBucket).formatArn({
      service: "ses",
      resource: "identity",
      resourceName: props.config.notificationEmail.identityDomain,
    })],
  }));
  functions.auraHistoriaApi.addToRolePolicy(new iam.PolicyStatement({
    actions: ["s3:GetObject"],
    resources: [props.mailTemplateBucket.arnForObjects(
      `${props.config.stage}/${props.parameters.commitSha}/mjml/newsletter/confirmation/*`,
    )],
  }));
  functions.auraHistoriaApi.addToRolePolicy(new iam.PolicyStatement({
    actions: ["ses:SendEmail"],
    resources: [cdk.Stack.of(props.mailTemplateBucket).formatArn({
      service: "ses",
      resource: "identity",
      resourceName: props.config.notificationEmail.identityDomain,
    })],
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
}

export function grantCognitoAdminAccess(functions: LambdaFunctions, userPoolArn: string): void {
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

function providerSignupEnvironment(context: LambdaEnvironmentContext): Record<string, string> {
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

function notificationDeliveryEnvironment(context: LambdaEnvironmentContext): Record<string, string> {
  return {
    COMMIT_SHA: context.commitSha,
    NOTIFICATION_EMAIL_FROM: context.config.notificationEmail.from,
    NOTIFICATION_EMAIL_REPLY_TO: context.config.notificationEmail.replyTo,
    S3_BUCKET_NAME_TEMPLATES: MAIL_TEMPLATE_BUCKET_NAME,
    STAGE: context.config.stage,
  };
}

function marketingConsentSyncEnvironment(context: LambdaEnvironmentContext): Record<string, string> {
  return loopsNewsletterEnvironment(context.config);
}

function loopsNewsletterEnvironment(config: StageConfig): Record<string, string> {
  return {
    LOOPS_API_BASE_URL: "https://app.loops.so/api",
    LOOPS_API_KEY: ssmValue(`/loops/${config.stage}/api-key`),
    LOOPS_NEWSLETTER_LIST_ID: ssmValue(`/loops/${config.stage}/newsletter-list-id`),
  };
}

function consentLogRetentionEnvironment(context: LambdaEnvironmentContext): Record<string, string> {
  return {
    CONSENT_EVIDENCE_LOG_GROUPS: JSON.stringify(consentEvidenceLogGroupNames(context.config.stage)),
  };
}

function apiEnvironment(context: LambdaEnvironmentContext): Record<string, string> {
  const { config, search } = context;
  const environment = {
    AWS_LAMBDA_HTTP_IGNORE_STAGE_IN_PATH: "true",
    COMMIT_SHA: context.commitSha,
    OPENSEARCH_ENDPOINT_URL: search.endpointUrl,
    PRODUCT_LISTING_INGESTION_QUEUE_URL: context.queues.productListingIngestion.queue.queueUrl,
    NEWSLETTER_CONFIRMATION_EMAIL_FROM: config.notificationEmail.from,
    NEWSLETTER_CONFIRMATION_EMAIL_REPLY_TO: config.notificationEmail.replyTo,
    NEWSLETTER_CONFIRMATION_FRONTEND_ORIGIN: config.isProd
      ? "https://aura-historia.com"
      : "https://stage.aura-historia.com",
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

export function importLambdaCatalog(scope: Construct, id: string, config: StageConfig): LambdaCatalog {
  const catalog = {} as Partial<Record<LambdaKey, lambda.IFunction>>;
  const importScope = new Construct(scope, id);

  for (const [key, definition] of Object.entries(LAMBDA_DEFINITIONS) as [LambdaKey, LambdaDefinition][]) {
    if (key === "auraHistoriaApi") {
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
