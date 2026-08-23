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
    environment: (context) =>
      withOpenSearchCredentials(context.config, {
        ...baseEnvironment(context),
        GEMINI_API_KEY: secretOrTest(context.config, "gemini-api-key", "test-key"),
        GOOGLE_GEOCODING_API_KEY: secretOrTest(context.config, "google-geocoding-api-key", "test-key"),
        OPENSEARCH_ENDPOINT_URL: context.search.endpointUrl,
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

function grantRuntimeAccess(props: LambdasProps, functions: LambdaFunctions): void {
  for (const [key, fn] of Object.entries(functions) as [LambdaKey, lambda.Function | undefined][]) {
    if (!fn || key === "cloudWatchLogRetention" || key === "fxRateSync") {
      continue;
    }

    props.table.grantReadWriteData(fn);
  }

  props.search.grantReadWrite(functions.shopify);
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

function secretOrTest(config: StageConfig, name: string, testValue: string): string {
  return config.isEphemeral ? testValue : ssmValue(`/secrets/${config.stage}/${name}`);
}
