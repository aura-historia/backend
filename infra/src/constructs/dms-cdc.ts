import * as cdk from "aws-cdk-lib";
import * as dms from "aws-cdk-lib/aws-dms";
import * as ec2 from "aws-cdk-lib/aws-ec2";
import * as iam from "aws-cdk-lib/aws-iam";
import * as kinesis from "aws-cdk-lib/aws-kinesis";
import { Construct } from "constructs";
import {
  DMS_CDC_INITIAL_START_POSITION_CONSTRAINT,
  DMS_CDC_INITIAL_START_POSITION_PARAMETER_LOGICAL_ID,
  DMS_CDC_INITIAL_START_POSITION_PATTERN,
  type StageConfig,
} from "../config";
import type { Network } from "./network";
import type { Storage } from "./storage";

export interface DmsCdcProps {
  readonly config: StageConfig;
  readonly network?: Network;
  readonly storage: Storage;
}

/** Private, CDC-only PostgreSQL-to-Kinesis contract. The consumer handoff is owned by #1787/#1788. */
export class DmsCdc extends Construct {
  readonly stream: kinesis.Stream;
  readonly replicationInstance: dms.CfnReplicationInstance;
  readonly sourceEndpoint: dms.CfnEndpoint;
  readonly targetEndpoint: dms.CfnEndpoint;
  readonly task: dms.CfnReplicationTask;

  constructor(scope: Construct, id: string, props: DmsCdcProps) {
    super(scope, id);

    const { config, network, storage } = props;
    if (!config.dms || !config.rds || !config.network || !network || !storage.database || !storage.adminCredentials || !storage.runtimeCredentials || !storage.migrationCredentials || !storage.replicationCredentials) {
      throw new Error("DMS CDC resources are only available in real AWS stages with PostgreSQL and private networking.");
    }

    const stage = config.stage;
    const dmsConfig = config.dms;
    const rdsConfig = config.rds;
    const sourceSecretsServicePrincipal = `dms.${config.network.region}.amazonaws.com`;
    const kinesisTargetServicePrincipal = "dms.amazonaws.com";
    const streamName = `aura-historia-cdc-${stage}`;

    this.stream = new kinesis.Stream(this, "CdcStream", {
      streamName,
      streamMode: kinesis.StreamMode.PROVISIONED,
      shardCount: 1,
      encryption: kinesis.StreamEncryption.MANAGED,
      retentionPeriod: cdk.Duration.days(dmsConfig.kinesisRetentionDays),
      removalPolicy: config.removalPolicy,
    });

    const kinesisEndpoint = new ec2.InterfaceVpcEndpoint(this, "KinesisInterfaceEndpoint", {
      vpc: network.vpc,
      service: ec2.InterfaceVpcEndpointAwsService.KINESIS_STREAMS,
      privateDnsEnabled: true,
      securityGroups: [network.dmsEndpointSecurityGroup],
      subnets: { subnetType: ec2.SubnetType.PRIVATE_WITH_EGRESS },
      open: false,
    });
    kinesisEndpoint.addToPolicy(new iam.PolicyStatement({
      principals: [new iam.AnyPrincipal()],
      actions: ["kinesis:DescribeStream", "kinesis:DescribeStreamSummary", "kinesis:PutRecord", "kinesis:PutRecords"],
      resources: [this.stream.streamArn],
    }));

    const secretsManagerEndpoint = new ec2.InterfaceVpcEndpoint(this, "SecretsManagerInterfaceEndpoint", {
      vpc: network.vpc,
      service: ec2.InterfaceVpcEndpointAwsService.SECRETS_MANAGER,
      privateDnsEnabled: true,
      securityGroups: [network.dmsEndpointSecurityGroup],
      subnets: { subnetType: ec2.SubnetType.PRIVATE_WITH_EGRESS },
      open: false,
    });
    secretsManagerEndpoint.addToPolicy(new iam.PolicyStatement({
      principals: [new iam.AnyPrincipal()],
      actions: ["secretsmanager:GetSecretValue"],
      resources: [
        storage.adminCredentials.secretArn,
        storage.runtimeCredentials.secretArn,
        storage.migrationCredentials.secretArn,
        storage.replicationCredentials.secretArn,
      ],
    }));
    secretsManagerEndpoint.addToPolicy(new iam.PolicyStatement({
      principals: [new iam.AnyPrincipal()],
      actions: ["secretsmanager:DescribeSecret"],
      resources: [storage.replicationCredentials.secretArn],
    }));

    const sourceSecretsRole = new iam.Role(this, "DmsSourceSecretsRole", {
      roleName: `aura-historia-dms-source-secrets-${stage}`,
      assumedBy: new iam.ServicePrincipal(sourceSecretsServicePrincipal),
      description: "AWS DMS reads only the generated PostgreSQL replication secret",
    });
    storage.replicationCredentials.grantRead(sourceSecretsRole);

    const kinesisTargetRole = new iam.Role(this, "DmsKinesisTargetRole", {
      roleName: `aura-historia-dms-kinesis-target-${stage}`,
      assumedBy: new iam.ServicePrincipal(kinesisTargetServicePrincipal),
      description: "AWS DMS writes only the Aura Historia CDC Kinesis stream",
    });
    kinesisTargetRole.addToPolicy(new iam.PolicyStatement({
      actions: ["kinesis:DescribeStream", "kinesis:DescribeStreamSummary", "kinesis:PutRecord", "kinesis:PutRecords"],
      resources: [this.stream.streamArn],
    }));

    const subnetGroup = new dms.CfnReplicationSubnetGroup(this, "ReplicationSubnetGroup", {
      replicationSubnetGroupIdentifier: `aura-historia-dms-${stage}`,
      replicationSubnetGroupDescription: "Aura Historia DMS private application subnets",
      subnetIds: network.vpc.selectSubnets({ subnetType: ec2.SubnetType.PRIVATE_WITH_EGRESS }).subnetIds,
      tags: [{ key: "Name", value: `aura-historia-dms-${stage}` }],
    });
    subnetGroup.applyRemovalPolicy(config.removalPolicy);

    this.replicationInstance = new dms.CfnReplicationInstance(this, "ReplicationInstance", {
      replicationInstanceIdentifier: `aura-historia-dms-cdc-${stage}`,
      replicationInstanceClass: dmsConfig.replicationInstanceClass,
      engineVersion: dmsConfig.engineVersion,
      replicationSubnetGroupIdentifier: subnetGroup.ref,
      vpcSecurityGroupIds: [network.dmsSecurityGroup.securityGroupId],
      publiclyAccessible: false,
      multiAz: false,
      autoMinorVersionUpgrade: false,
      tags: [{ key: "Name", value: `aura-historia-dms-cdc-${stage}` }],
    });
    this.replicationInstance.applyRemovalPolicy(config.removalPolicy);

    this.sourceEndpoint = new dms.CfnEndpoint(this, "PostgresSourceEndpoint", {
      databaseName: rdsConfig.databaseName,
      endpointIdentifier: `aura-historia-postgres-cdc-${stage}`,
      endpointType: "source",
      engineName: "postgres",
      sslMode: "require",
      postgreSqlSettings: {
        captureDdls: false,
        failTasksOnLobTruncation: true,
        pluginName: "test-decoding",
        secretsManagerAccessRoleArn: sourceSecretsRole.roleArn,
        secretsManagerSecretId: storage.replicationCredentials.secretArn,
        slotName: `aura_historia_dms_cdc_${stage}`,
      },
    });
    this.sourceEndpoint.applyRemovalPolicy(config.removalPolicy);

    this.targetEndpoint = new dms.CfnEndpoint(this, "KinesisTargetEndpoint", {
      endpointIdentifier: `aura-historia-kinesis-cdc-${stage}`,
      endpointType: "target",
      engineName: "kinesis",
      kinesisSettings: {
        includeControlDetails: true,
        includeNullAndEmpty: true,
        includeTableAlterOperations: true,
        includeTransactionDetails: true,
        messageFormat: "JSON",
        serviceAccessRoleArn: kinesisTargetRole.roleArn,
        streamArn: this.stream.streamArn,
      },
    });
    this.targetEndpoint.applyRemovalPolicy(config.removalPolicy);

    const initialCdcStartPosition = new cdk.CfnParameter(this, dmsConfig.initialCdcStartPositionParameterId, {
      type: "String",
      default: "",
      allowedPattern: DMS_CDC_INITIAL_START_POSITION_PATTERN,
      constraintDescription: DMS_CDC_INITIAL_START_POSITION_CONSTRAINT,
      description: "Optional compatibility LSN. Empty declares an unstarted greenfield task; existing stacks retain their prior approved first-start LSN.",
    });
    initialCdcStartPosition.overrideLogicalId(DMS_CDC_INITIAL_START_POSITION_PARAMETER_LOGICAL_ID);
    const hasInitialCdcStartPosition = new cdk.CfnCondition(this, "HasInitialCdcStartPosition", {
      expression: cdk.Fn.conditionNot(cdk.Fn.conditionEquals(initialCdcStartPosition.valueAsString, "")),
    });

    this.task = new dms.CfnReplicationTask(this, "CdcTask", {
      replicationTaskIdentifier: `aura-historia-cdc-${stage}`,
      migrationType: "cdc",
      cdcStartPosition: cdk.Fn.conditionIf(
        hasInitialCdcStartPosition.logicalId,
        initialCdcStartPosition.valueAsString,
        cdk.Aws.NO_VALUE,
      ) as unknown as string,
      replicationInstanceArn: this.replicationInstance.ref,
      sourceEndpointArn: this.sourceEndpoint.attrEndpointArn,
      targetEndpointArn: this.targetEndpoint.attrEndpointArn,
      replicationTaskSettings: JSON.stringify(replicationTaskSettings(dmsConfig.lobMaxSizeKiB)),
      tableMappings: JSON.stringify(tableMappings()),
      tags: [{ key: "Name", value: `aura-historia-cdc-${stage}` }],
    });
    this.task.applyRemovalPolicy(config.removalPolicy);
  }
}

function replicationTaskSettings(lobMaxSizeKiB: number): Record<string, unknown> {
  return {
    FullLoadSettings: {
      TargetTablePrepMode: "DO_NOTHING",
    },
    TargetMetadata: {
      BatchApplyEnabled: false,
      FullLobMode: false,
      LimitedSizeLobMode: true,
      LobMaxSize: lobMaxSizeKiB,
      SupportLobs: true,
      TargetSchema: "",
    },
  };
}

function tableMappings(): Record<string, unknown> {
  // R5 activates only the ProductListing event journal. The strict router retains
  // the complete future catalog, but R6 owns enabling and proving other tables.
  const selectedTables = [
    {
      table: "product_listing_events",
      removedColumns: ["created"],
    },
  ];

  let ruleId = 1;
  const nextRule = (): string => String(ruleId++);
  const rules: Record<string, unknown>[] = selectedTables.map(({ table }) => ({
    "rule-type": "selection",
    "rule-id": nextRule(),
    "rule-name": `include-${table}`,
    "object-locator": {
      "schema-name": "public",
      "table-name": table,
    },
    "rule-action": "include",
  }));

  for (const { table, removedColumns } of selectedTables) {
    for (const column of removedColumns) {
      rules.push({
        "rule-type": "transformation",
        "rule-id": nextRule(),
        "rule-name": `remove-${table}-${column}`,
        "rule-target": "column",
        "object-locator": {
          "schema-name": "public",
          "table-name": table,
          "column-name": column,
        },
        "rule-action": "remove-column",
      });
    }
  }

  const selectedTableNames = new Set(selectedTables.map(({ table }) => table));
  for (const { table, column } of [
    { table: "product_listing_raw_revisions", column: "revision" },
    { table: "search_filters", column: "version" },
  ]) {
    if (!selectedTableNames.has(table)) {
      continue;
    }
    rules.push({
      "rule-type": "transformation",
      "rule-id": nextRule(),
      "rule-name": `decimal-string-${table}-${column}`,
      "rule-target": "column",
      "object-locator": {
        "schema-name": "public",
        "table-name": table,
        "column-name": column,
      },
      "rule-action": "change-data-type",
      "data-type": {
        type: "string",
        length: 19,
      },
    });
  }

  return { rules };
}
