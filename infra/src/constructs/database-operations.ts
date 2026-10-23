import * as cdk from "aws-cdk-lib";
import * as ec2 from "aws-cdk-lib/aws-ec2";
import * as iam from "aws-cdk-lib/aws-iam";
import * as lambda from "aws-cdk-lib/aws-lambda";
import * as path from "node:path";
import * as s3 from "aws-cdk-lib/aws-s3";
import { Construct } from "constructs";
import { ssmValue, type StageConfig } from "../config";
import type { Network } from "./network";
import type { PostgresConnectionSettings, PostgresMigrationConnectionSettings } from "./storage";
import { withPostgresEnvironment } from "./lambdas";

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

