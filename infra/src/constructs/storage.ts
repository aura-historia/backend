import * as dynamodb from "aws-cdk-lib/aws-dynamodb";
import { Construct } from "constructs";
import type { StageConfig } from "../config";
import { ssmValue } from "../config";

export interface StorageProps {
  readonly config: StageConfig;
  readonly stageName: string;
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
  readonly table: dynamodb.Table;

  constructor(scope: Construct, id: string, props: StorageProps) {
    super(scope, id);

    this.postgres = postgresConnectionSettings(props.config);

    this.table = new dynamodb.Table(this, "TableOne", {
      tableName: `table_1-${props.stageName}`,
      partitionKey: { name: "pk", type: dynamodb.AttributeType.STRING },
      sortKey: { name: "sk", type: dynamodb.AttributeType.STRING },
      billingMode: dynamodb.BillingMode.PAY_PER_REQUEST,
      stream: dynamodb.StreamViewType.NEW_IMAGE,
      timeToLiveAttribute: "ttl",
      tableClass: dynamodb.TableClass.STANDARD,
      removalPolicy: props.config.removalPolicy,
    });

    this.table.addLocalSecondaryIndex({
      indexName: "lsi1",
      sortKey: { name: "lsi1_sk", type: dynamodb.AttributeType.STRING },
      projectionType: dynamodb.ProjectionType.ALL,
    });
    this.table.addLocalSecondaryIndex({
      indexName: "lsi2",
      sortKey: { name: "lsi2_sk", type: dynamodb.AttributeType.STRING },
      projectionType: dynamodb.ProjectionType.ALL,
    });
    this.table.addLocalSecondaryIndex({
      indexName: "lsi3",
      sortKey: { name: "lsi3_sk", type: dynamodb.AttributeType.STRING },
      projectionType: dynamodb.ProjectionType.ALL,
    });
    this.table.addLocalSecondaryIndex({
      indexName: "lsi4",
      sortKey: { name: "lsi4_sk", type: dynamodb.AttributeType.STRING },
      projectionType: dynamodb.ProjectionType.KEYS_ONLY,
    });
    this.table.addLocalSecondaryIndex({
      indexName: "lsi5",
      sortKey: { name: "lsi5_sk", type: dynamodb.AttributeType.STRING },
      projectionType: dynamodb.ProjectionType.KEYS_ONLY,
    });

    this.table.addGlobalSecondaryIndex({
      indexName: "gsi1",
      partitionKey: { name: "gsi1_pk", type: dynamodb.AttributeType.STRING },
      sortKey: { name: "gsi1_sk", type: dynamodb.AttributeType.STRING },
      projectionType: dynamodb.ProjectionType.ALL,
    });
    this.table.addGlobalSecondaryIndex({
      indexName: "gsi2",
      partitionKey: { name: "gsi2_pk", type: dynamodb.AttributeType.STRING },
      sortKey: { name: "gsi2_sk", type: dynamodb.AttributeType.STRING },
      projectionType: dynamodb.ProjectionType.KEYS_ONLY,
    });
    this.table.addGlobalSecondaryIndex({
      indexName: "gsi3",
      partitionKey: { name: "gsi3_pk", type: dynamodb.AttributeType.STRING },
      sortKey: { name: "gsi3_sk", type: dynamodb.AttributeType.STRING },
      projectionType: dynamodb.ProjectionType.ALL,
    });
  }
}

function postgresConnectionSettings(config: StageConfig): PostgresConnectionSettings {
  if (config.isEphemeral) {
    return {
      host: "host.docker.internal",
      port: "5432",
      database: "postgres",
      username: "postgres",
      password: "postgres",
      maxConnections: "2",
    };
  }

  return {
    host: ssmValue(`/postgres/${config.stage}/host`),
    port: ssmValue(`/postgres/${config.stage}/port`),
    database: ssmValue(`/postgres/${config.stage}/database`),
    username: ssmValue(`/postgres/${config.stage}/username`),
    password: ssmValue(`/secrets/${config.stage}/postgres-password`),
    maxConnections: "2",
  };
}
