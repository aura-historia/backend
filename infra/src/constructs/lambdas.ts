import * as cdk from "aws-cdk-lib";
import * as iam from "aws-cdk-lib/aws-iam";
import * as lambda from "aws-cdk-lib/aws-lambda";
import * as s3 from "aws-cdk-lib/aws-s3";
import { Construct } from "constructs";
import type { StageConfig, StageName } from "../config";
import { ssmValue } from "../config";
import type { ApplicationParameters } from "../parameters";

import type { PostgresConnectionSettings } from "./storage";
import type { LambdaEgressOutput } from "./lambda-egress";
import type { PostgresLambdaConfig } from "../postgres-lambda-config";
import { POSTGRES_CA_PATH, PostgresCa } from "./postgres-ca";

interface LambdaEnvironmentContext {
  readonly config: StageConfig;
  readonly postgres: PostgresConnectionSettings;
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
export type DatabaseLambdaKey = {
  [K in LambdaKey]: typeof LAMBDA_DEFINITIONS[K] extends { readonly postgres: true } ? K : never
}[LambdaKey];
export const DATABASE_LAMBDA_KEYS: readonly DatabaseLambdaKey[] = Object.entries(LAMBDA_DEFINITIONS)
  .filter(([, definition]) => "postgres" in definition && definition.postgres)
  .map(([key]) => key as DatabaseLambdaKey);
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
  readonly postgresLambda?: { readonly config: PostgresLambdaConfig; readonly egress: LambdaEgressOutput };
}

export class Lambdas extends Construct {
  readonly functions: LambdaFunctions;

  constructor(scope: Construct, id: string, props: LambdasProps) {
    super(scope, id);

    const functions = {} as Partial<Record<LambdaKey, lambda.Function>>;
    const attachment = props.postgresLambda;
    if (attachment && (props.config.isEphemeral || attachment.config.stage !== props.config.stage
      || attachment.egress.stage !== props.config.stage)) {
      throw new Error("PostgreSQL Lambda attachment stage mismatch.");
    }
    const ca = attachment ? new PostgresCa(this, "PostgresCa", {
      assetDirectory: attachment.config.caAssetDirectory,
    }) : undefined;
    const environmentContext: LambdaEnvironmentContext = {
      config: props.config,
      postgres: attachment ? {
        ...props.postgres,
        host: attachment.config.databaseHostname,
        port: String(attachment.config.network.database.port),
        maxConnections: "2",
        sslMode: "verify-full",
        sslRootCert: POSTGRES_CA_PATH,
      } : props.postgres,
    };

    for (const [key, definition] of Object.entries(LAMBDA_DEFINITIONS) as [LambdaKey, LambdaDefinition][]) {
      if (props.config.isEphemeral && definition.skipEphemeral) {
        continue;
      }

      const databaseAttachment = definition.postgres ? attachment : undefined;
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
        ...(databaseAttachment ? {
          vpc: databaseAttachment.egress.vpc,
          vpcSubnets: { subnets: [...databaseAttachment.egress.privateSubnets] },
          securityGroups: [databaseAttachment.egress.securityGroup],
          layers: [ca!.layer],
          reservedConcurrentExecutions: databaseAttachment.config.reservedConcurrency[key as DatabaseLambdaKey],
        } : {}),
      });
      if (databaseAttachment) {
        // Lambda service needs ENI permissions; function code itself does not.
        // Literal name avoids a role -> function -> role dependency cycle.
        functions[key]!.addToRolePolicy(new iam.PolicyStatement({
          effect: iam.Effect.DENY,
          actions: [
            "ec2:CreateNetworkInterface", "ec2:DeleteNetworkInterface", "ec2:DescribeNetworkInterfaces",
            "ec2:DescribeSubnets", "ec2:DetachNetworkInterface",
            "ec2:AssignPrivateIpAddresses", "ec2:UnassignPrivateIpAddresses",
          ],
          resources: ["*"],
          conditions: { ArnEquals: { "lambda:SourceFunctionArn": cdk.Stack.of(this).formatArn({
            service: "lambda", resource: "function", resourceName: lambdaFunctionName(key, props.config.stage),
            arnFormat: cdk.ArnFormat.COLON_RESOURCE_NAME,
          }) } },
        }));
      }
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
    POSTGRES_USERNAME: context.postgres.username,
    STAGE: context.config.stage,
    POSTGRES_SSL_MODE: context.postgres.sslMode,
    ...(context.postgres.sslRootCert ? { POSTGRES_SSL_ROOT_CERT: context.postgres.sslRootCert } : {}),
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
  _functions: LambdaFunctions,
  _userPoolId: string,
  _publicClientId: string,
): void {}

export function grantCognitoAdminAccess(_functions: LambdaFunctions, _userPoolArn: string): void {}

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
