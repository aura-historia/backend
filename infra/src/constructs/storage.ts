import * as cdk from "aws-cdk-lib";
import * as ec2 from "aws-cdk-lib/aws-ec2";
import * as rds from "aws-cdk-lib/aws-rds";
import { Construct } from "constructs";
import type { StageConfig } from "../config";
import type { Network } from "./network";

export interface StorageProps {
  readonly config: StageConfig;
  readonly network?: Network;
}

export interface PostgresConnectionSettings {
  readonly host: string;
  readonly port: string;
  readonly database: string;
  readonly username: string;
  readonly password: string;
  readonly maxConnections: string;
}

export class Storage extends Construct {
  readonly postgres: PostgresConnectionSettings;
  readonly database?: rds.DatabaseInstance;
  readonly runtimeCredentials?: rds.DatabaseSecret;
  readonly migrationCredentials?: rds.DatabaseSecret;
  readonly replicationCredentials?: rds.DatabaseSecret;

  constructor(scope: Construct, id: string, props: StorageProps) {
    super(scope, id);

    if (props.config.isEphemeral) {
      this.postgres = localPostgresConnectionSettings();
      return;
    }

    if (!props.config.rds || !props.network) {
      throw new Error("Real AWS stages require RDS and network configuration.");
    }

    const rdsConfig = props.config.rds;
    const parameterGroup = new rds.ParameterGroup(this, "PostgresParameterGroup", {

      engine: postgresEngine(rdsConfig.engineVersion),
      description: `Aura Historia PostgreSQL ${rdsConfig.engineVersion} policy`,
      parameters: {
        "rds.force_ssl": "1",
        "rds.logical_replication": "1",
        "max_replication_slots": "5",
        "max_wal_senders": "5",
        "max_slot_wal_keep_size": "10240",
      },
    });
    const subnetGroup = new rds.SubnetGroup(this, "PostgresSubnetGroup", {
      description: "Aura Historia isolated PostgreSQL subnets",
      subnetGroupName: `aura-historia-postgres-${props.config.stage}`,
      vpc: props.network.vpc,
      vpcSubnets: { subnetType: ec2.SubnetType.PRIVATE_ISOLATED },
    });
    const adminCredentials = new rds.DatabaseSecret(this, "PostgresAdminCredentials", {
      username: "aura_admin",
      secretName: `/aura-historia/${props.config.stage}/postgres/admin`,
    });

    this.database = new rds.DatabaseInstance(this, "Postgres", {
      instanceIdentifier: `aura-historia-postgres-${props.config.stage}`,
      engine: postgresEngine(rdsConfig.engineVersion),
      credentials: rds.Credentials.fromSecret(adminCredentials),
      databaseName: rdsConfig.databaseName,
      instanceType: new ec2.InstanceType(rdsConfig.instanceType),
      vpc: props.network.vpc,
      subnetGroup,
      securityGroups: [props.network.databaseSecurityGroup],
      parameterGroup,
      multiAz: false,
      publiclyAccessible: false,
      storageType: rds.StorageType.GP3,
      allocatedStorage: rdsConfig.allocatedStorageGiB,
      maxAllocatedStorage: rdsConfig.maxAllocatedStorageGiB,
      storageEncrypted: true,
      backupRetention: cdk.Duration.days(rdsConfig.backupRetentionDays),
      preferredBackupWindow: "02:00-02:30",
      preferredMaintenanceWindow: "sun:03:00-sun:03:30",
      autoMinorVersionUpgrade: true,
      cloudwatchLogsExports: ["postgresql"],
      copyTagsToSnapshot: true,
      deleteAutomatedBackups: !props.config.isProd,
      deletionProtection: props.config.isProd,
      removalPolicy: props.config.removalPolicy,
    });

    this.runtimeCredentials = applicationCredentials(this, "PostgresRuntimeCredentials", {
      stage: props.config.stage,
      username: "aura_runtime",
      databaseName: rdsConfig.databaseName,
      adminCredentials,
    });
    this.migrationCredentials = applicationCredentials(this, "PostgresMigrationCredentials", {
      stage: props.config.stage,
      username: "aura_migrator",
      databaseName: rdsConfig.databaseName,
      adminCredentials,
    });
    this.replicationCredentials = applicationCredentials(this, "PostgresReplicationCredentials", {
      stage: props.config.stage,
      username: "aura_replication",
      databaseName: rdsConfig.databaseName,
      adminCredentials,
    });

    this.postgres = {
      host: this.database.dbInstanceEndpointAddress,
      port: this.database.dbInstanceEndpointPort,
      database: rdsConfig.databaseName,
      username: this.runtimeCredentials.secretValueFromJson("username").unsafeUnwrap(),
      password: this.runtimeCredentials.secretValueFromJson("password").unsafeUnwrap(),
      maxConnections: "2",
    };
  }
}

interface ApplicationCredentialProps {
  readonly stage: string;
  readonly username: string;
  readonly databaseName: string;
  readonly adminCredentials: rds.DatabaseSecret;
}

function applicationCredentials(scope: Construct, id: string, props: ApplicationCredentialProps): rds.DatabaseSecret {
  return new rds.DatabaseSecret(scope, id, {
    username: props.username,
    dbname: props.databaseName,
    secretName: `/aura-historia/${props.stage}/postgres/${roleSecretName(props.username)}`,
    masterSecret: props.adminCredentials,
  });
}

function roleSecretName(username: string): string {
  return username.replace("aura_", "");
}

function postgresEngine(version: "16.13"): rds.IInstanceEngine {
  switch (version) {
    case "16.13":
      return rds.DatabaseInstanceEngine.postgres({ version: rds.PostgresEngineVersion.VER_16_13 });
  }
}

function localPostgresConnectionSettings(): PostgresConnectionSettings {
  return {
    host: "host.docker.internal",
    port: "5432",
    database: "postgres",
    username: "postgres",
    password: "postgres",
    maxConnections: "2",
  };
}
