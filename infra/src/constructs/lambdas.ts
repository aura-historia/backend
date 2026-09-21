import * as cdk from "aws-cdk-lib";
import * as ec2 from "aws-cdk-lib/aws-ec2";
import * as iam from "aws-cdk-lib/aws-iam";
import * as lambda from "aws-cdk-lib/aws-lambda";
import * as s3 from "aws-cdk-lib/aws-s3";
import { Construct } from "constructs";
import type { StageConfig, StageName } from "../config";
import { ssmValue } from "../config";
import type { ApplicationParameters } from "../parameters";

import type { Network } from "./network";
import type { Search } from "./opensearch";
import type { PostgresConnectionSettings } from "./storage";

interface LambdaEnvironmentContext {
  readonly config: StageConfig;
  readonly postgres: PostgresConnectionSettings;
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
  },
  postConfirmation: {
    id: "PrimaryUserPoolPostConfirmationLambda",
    binaryName: "cognito-post-confirmation",
    memorySize: 256,
    postgres: true,
    timeoutSeconds: 5,
  },
  shopify: {
    id: "ShopifyLambda",
    binaryName: "shopify-lambda",
    memorySize: 256,
    postgres: true,
    timeoutSeconds: 30,
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
  readonly postgres: PostgresConnectionSettings;
  readonly search: Search;
  readonly network?: Network;
}

export class Lambdas extends Construct {
  readonly functions: LambdaFunctions;

  constructor(scope: Construct, id: string, props: LambdasProps) {
    super(scope, id);

    const functions = {} as Partial<Record<LambdaKey, lambda.Function>>;
    const environmentContext: LambdaEnvironmentContext = {
      config: props.config,
      postgres: props.postgres,
      search: props.search,
    };

    for (const [key, definition] of Object.entries(LAMBDA_DEFINITIONS) as [LambdaKey, LambdaDefinition][]) {
      if (props.config.isEphemeral && definition.skipEphemeral) {
        continue;
      }

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
  const env = definition.environment?.(context) ?? {};
  return definition.postgres ? withPostgresEnvironment(context, env) : env;
}

function withPostgresEnvironment(context: LambdaEnvironmentContext, env: Record<string, string>): Record<string, string> {
  return {
    ...env,
    POSTGRES_DATABASE: context.postgres.database,
    POSTGRES_HOST: context.postgres.host,
    POSTGRES_MAX_CONNECTIONS: context.postgres.maxConnections,
    POSTGRES_PASSWORD: context.postgres.password,
    POSTGRES_PORT: context.postgres.port,
    POSTGRES_TLS_ROOT_CERT: context.postgres.tlsRootCert,
    POSTGRES_USERNAME: context.postgres.username,
  };
}

function grantRuntimeAccess(_props: LambdasProps, functions: LambdaFunctions): void {
  functions.cloudWatchLogRetention.addToRolePolicy(
    new iam.PolicyStatement({
      actions: ["logs:DescribeLogGroups", "logs:PutRetentionPolicy"],
      resources: ["*"],
    }),
  );
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

function apiEnvironment(context: LambdaEnvironmentContext): Record<string, string> {
  const { config, search } = context;
  const environment = {
    AWS_LAMBDA_HTTP_IGNORE_STAGE_IN_PATH: "true",
    OPENSEARCH_ENDPOINT_URL: search.endpointUrl,
    STAGE: config.stage,
    STRIPE_CHECKOUT_CANCEL_URL: config.stripeCheckoutCancelUrl,
    STRIPE_CHECKOUT_SUCCESS_URL: config.stripeCheckoutSuccessUrl,
    STRIPE_PORTAL_RETURN_URL: config.stripePortalReturnUrl,
    STRIPE_PRO_MONTHLY_PRICE_ID: config.stripeProMonthlyPriceId,
    STRIPE_PRO_YEARLY_PRICE_ID: config.stripeProYearlyPriceId,
    STRIPE_ULTIMATE_MONTHLY_PRICE_ID: config.stripeUltimateMonthlyPriceId,
    STRIPE_ULTIMATE_YEARLY_PRICE_ID: config.stripeUltimateYearlyPriceId,
  };

  if (config.isEphemeral) {
    return {
      ...environment,
      STRIPE_API_KEY: "sk_test_ephemeral",
      ZOHO_ACCOUNTS_URL: "https://accounts.zoho.test",
      ZOHO_CAMPAIGNS_URL: "https://campaigns.zoho.test",
      ZOHO_CLIENT_ID: "ephemeral-client-id",
      ZOHO_CLIENT_SECRET: "ephemeral-client-secret",
      ZOHO_LIST_KEY: "ephemeral-list-key",
      ZOHO_REFRESH_TOKEN: "ephemeral-refresh-token",
    };
  }

  return {
    ...environment,
    OPENSEARCH_PASSWORD: ssmValue(`/opensearch/${config.stage}/password`),
    OPENSEARCH_USERNAME: ssmValue(`/opensearch/${config.stage}/username`),
    STRIPE_API_KEY: ssmValue(`/stripe/${config.stage}/api-key`),
    ZOHO_ACCOUNTS_URL: ssmValue(`/zoho/${config.stage}/accounts-url`),
    ZOHO_CAMPAIGNS_URL: ssmValue(`/zoho/${config.stage}/campaigns-url`),
    ZOHO_CLIENT_ID: ssmValue(`/zoho/${config.stage}/client-id`),
    ZOHO_CLIENT_SECRET: ssmValue(`/zoho/${config.stage}/client-secret`),
    ZOHO_LIST_KEY: ssmValue(`/zoho/${config.stage}/list-key`),
    ZOHO_REFRESH_TOKEN: ssmValue(`/zoho/${config.stage}/refresh-token`),
  };
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
