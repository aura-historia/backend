import * as cdk from "aws-cdk-lib";
import * as dynamodb from "aws-cdk-lib/aws-dynamodb";
import { Construct } from "constructs";
import type { StageConfig } from "../config";

export interface StorageProps {
  readonly config: StageConfig;
  readonly stageName: string;
}

export class Storage extends Construct {
  readonly table: dynamodb.Table;

  constructor(scope: Construct, id: string, props: StorageProps) {
    super(scope, id);

    this.table = new dynamodb.Table(this, "TableOne", {
      tableName: cdk.Fn.sub("table_1-${StageName}"),
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
