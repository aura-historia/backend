import * as cdk from "aws-cdk-lib";
import * as ec2 from "aws-cdk-lib/aws-ec2";
import * as iam from "aws-cdk-lib/aws-iam";
import * as lambda from "aws-cdk-lib/aws-lambda";
import * as path from "node:path";
import type * as s3 from "aws-cdk-lib/aws-s3";
import { Construct } from "constructs";
import type { StageConfig } from "../config";
import { credentialResolver, credentialSecrets } from "../credentials";
import { releaseLambdaCode, lambdaArtifactParameters } from "../release-artifacts";
import type { Network } from "./network";
import type { PostgresConnectionSettings, PostgresMigrationConnectionSettings } from "./storage";
import { LAMBDA_LOGGING, lambdaLogGroup, lambdaLogLevel } from "./lambda-logging";
import { withPostgresEnvironment } from "./lambdas";

export interface DatabaseOperationsProps {
  readonly config: StageConfig;
  readonly artifactBucket: s3.IBucket;
  readonly migrationPostgres: PostgresMigrationConnectionSettings;
  readonly postgres: PostgresConnectionSettings;
  readonly network: Network;
}

/** Candidate code is deployed before invoking its fixed, non-switchable operation. */
export class RoutineDatabaseOperations extends Construct {
  readonly databaseMigration: lambda.Function;
  readonly migrationVersion: lambda.Version;

  constructor(scope: Construct, id: string, props: DatabaseOperationsProps) {
    super(scope, id);
    const code = releaseLambdaCode(cdk.Stack.of(this), props.artifactBucket, "database-migration-lambda");
    const layer = rootCertificateLayer(this);
    this.databaseMigration = databaseFunction(this, "DatabaseMigrationLambda", props, code, layer, "MIGRATE");
    this.migrationVersion = this.databaseMigration.currentVersion;
    new cdk.CfnOutput(cdk.Stack.of(this), "DatabaseMigrationVersionArn", { value: this.migrationVersion.functionArn });
  }
}

/** Explicit maintenance bootstrap and initial FX capture have no routine invocation permission. */
export class InitializationLambdas extends Construct {
  readonly databaseMigration: lambda.Function;
  readonly databaseInspection: lambda.Function;

  constructor(scope: Construct, id: string, props: DatabaseOperationsProps) {
    super(scope, id);
    const layer = rootCertificateLayer(this);
    this.databaseMigration = databaseFunction(this, "DatabaseBootstrapLambda", props,
      releaseLambdaCode(cdk.Stack.of(this), props.artifactBucket, "database-migration-lambda"), layer, "BOOTSTRAP");
    this.databaseInspection = databaseFunction(this, "DatabaseInspectionLambda", props,
      releaseLambdaCode(cdk.Stack.of(this), props.artifactBucket, "database-migration-lambda"), layer, "INSPECT");
    new cdk.CfnOutput(cdk.Stack.of(this), "DatabaseInspectionVersionArn", { value: this.databaseInspection.currentVersion.functionArn });
    const credential = credentialResolver(this);
    new lambda.Function(this, "FxRateSyncLambda", {
      vpc: props.network.vpc,
      vpcSubnets: { subnetType: ec2.SubnetType.PRIVATE_WITH_EGRESS },
      securityGroups: [props.network.applicationSecurityGroup],
      functionName: `fxrate-lambda-${props.config.stage}`,
      logGroup: lambdaLogGroup(this, "FxRateSyncLogGroup", `fxrate-lambda-${props.config.stage}`, props.config),
      ...LAMBDA_LOGGING,
      runtime: lambda.Runtime.PROVIDED_AL2023,
      architecture: lambda.Architecture.X86_64,
      handler: "lib.handler",
      code: releaseLambdaCode(cdk.Stack.of(this), props.artifactBucket, "fxrate-lambda"),
      memorySize: 128,
      timeout: cdk.Duration.seconds(10),
      environment: withPostgresEnvironment({ config: props.config, postgres: props.postgres, credential }, {
        LOG_LEVEL: lambdaLogLevel(props.config),
        FXRATES_API_TOKEN: credential(credentialSecrets(props.config.stage).fxRatesApi, "apiToken"),
      }),
      layers: [layer],
    });
  }
}

function databaseFunction(scope: Construct, id: string, props: DatabaseOperationsProps,
  code: lambda.Code, layer: lambda.LayerVersion, operation: "MIGRATE" | "INSPECT" | "BOOTSTRAP"): lambda.Function {
  const postgres = props.migrationPostgres;
  const operationSecrets: Record<typeof operation, Record<string, string>> = {
    MIGRATE: { POSTGRES_MIGRATION_SECRET_ARN: postgres.migrationSecretArn },
    INSPECT: { POSTGRES_INSPECTION_SECRET_ARN: postgres.inspectionSecretArn },
    BOOTSTRAP: {
      POSTGRES_ADMIN_SECRET_ARN: postgres.adminSecretArn,
      POSTGRES_RUNTIME_SECRET_ARN: postgres.runtimeSecretArn,
      POSTGRES_MIGRATION_SECRET_ARN: postgres.migrationSecretArn,
      POSTGRES_REPLICATION_SECRET_ARN: postgres.replicationSecretArn,
      POSTGRES_INSPECTION_SECRET_ARN: postgres.inspectionSecretArn,
    },
  };
  const names = { MIGRATE: "migration", INSPECT: "inspection", BOOTSTRAP: "bootstrap" };
  const functionName = `database-${names[operation]}-lambda-${props.config.stage}`;
  const fn = new lambda.Function(scope, id, {
    vpc: props.network.vpc,
    vpcSubnets: { subnetType: ec2.SubnetType.PRIVATE_WITH_EGRESS },
    securityGroups: [props.network.migrationSecurityGroup],
    functionName,
    logGroup: lambdaLogGroup(scope, `${id}LogGroup`, functionName, props.config),
    ...LAMBDA_LOGGING,
    runtime: lambda.Runtime.PROVIDED_AL2023,
    architecture: lambda.Architecture.X86_64,
    handler: "lib.handler", code,
    currentVersionOptions: { description: lambdaArtifactParameters(cdk.Stack.of(scope), "database-migration-lambda").sha256 },
    memorySize: 512,
    timeout: cdk.Duration.seconds(operation === "INSPECT" ? 60 : 840),
    environment: {
      LOG_LEVEL: lambdaLogLevel(props.config),
      DATABASE_OPERATION: operation, DATABASE_TARGET: props.config.stage,
      POSTGRES_DATABASE: postgres.database, POSTGRES_HOST: postgres.host,
      POSTGRES_MAX_CONNECTIONS: postgres.maxConnections, POSTGRES_PORT: postgres.port,
      POSTGRES_TLS_ROOT_CERT: postgres.tlsRootCert, ...operationSecrets[operation],
    },
    layers: [layer],
  });
  grantSecret(fn, Object.values(operationSecrets[operation]));
  return fn;
}

function grantSecret(fn: lambda.Function, resources: string[]): void {
  fn.addToRolePolicy(new iam.PolicyStatement({ actions: ["secretsmanager:GetSecretValue"], resources }));
}

function rootCertificateLayer(scope: Construct): lambda.LayerVersion {
  return new lambda.LayerVersion(scope, "PostgresTlsRootCertificateLayer", {
    code: lambda.Code.fromAsset(path.join(__dirname, "../../assets/rds-ca-layer")),
    compatibleRuntimes: [lambda.Runtime.PROVIDED_AL2023],
    description: "Public AWS RDS root certificate bundle for PostgreSQL Lambdas",
  });
}
