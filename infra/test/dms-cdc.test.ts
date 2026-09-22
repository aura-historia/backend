import * as cdk from "aws-cdk-lib";
import { Template } from "aws-cdk-lib/assertions";
import { createApplicationStacks } from "../src/application-stack";
import {
  DMS_CDC_INITIAL_START_POSITION_CONSTRAINT,
  DMS_CDC_INITIAL_START_POSITION_PARAMETER_LOGICAL_ID,
  DMS_CDC_INITIAL_START_POSITION_PATTERN,
  type StageName,
} from "../src/config";

const REAL_STAGES = ["dev", "prod"] as const;

function dataTemplate(stage: StageName): Template {
  const app = new cdk.App({ analyticsReporting: false });
  return Template.fromStack(createApplicationStacks(app, { stage }).data);
}

function resourceProperties(template: Template, type: string): Record<string, unknown>[] {
  return Object.values(template.findResources(type)).map((resource) => resource.Properties as Record<string, unknown>);
}


describe.each(REAL_STAGES)("%s private DMS CDC", (stage) => {
  test("declares one private provisioned DMS instance and a stopped CDC task without a first-start position", () => {
    const template = dataTemplate(stage);
    const instances = template.findResources("AWS::DMS::ReplicationInstance");
    const [instanceId] = Object.keys(instances);
    const [instance] = resourceProperties(template, "AWS::DMS::ReplicationInstance");
    const [task] = resourceProperties(template, "AWS::DMS::ReplicationTask");
    const source = resourceProperties(template, "AWS::DMS::Endpoint")
      .find((endpoint) => endpoint.EndpointType === "source");
    const streams = resourceProperties(template, "AWS::Kinesis::Stream");

    const startPositionParameter = template.findParameters("*")[DMS_CDC_INITIAL_START_POSITION_PARAMETER_LOGICAL_ID];
    expect(startPositionParameter).toMatchObject({
      Type: "String",
      Default: "",
      Description: "Optional compatibility LSN. Empty declares an unstarted greenfield task; existing stacks retain their prior approved first-start LSN.",
      AllowedPattern: DMS_CDC_INITIAL_START_POSITION_PATTERN,
      ConstraintDescription: DMS_CDC_INITIAL_START_POSITION_CONSTRAINT,
    });

    expect(instanceId).toBeDefined();
    expect(instance).toMatchObject({
      ReplicationInstanceIdentifier: `aura-historia-dms-cdc-${stage}`,
      ReplicationInstanceClass: "dms.t3.small",
      EngineVersion: "3.6.1",
      PubliclyAccessible: false,
      MultiAZ: false,
      AutoMinorVersionUpgrade: false,
    });
    expect(instance.VpcSecurityGroupIds).toHaveLength(1);
    expect(task).toMatchObject({
      ReplicationTaskIdentifier: `aura-historia-cdc-${stage}`,
      MigrationType: "cdc",
      CdcStartPosition: expect.objectContaining({
        "Fn::If": expect.arrayContaining([
          { Ref: DMS_CDC_INITIAL_START_POSITION_PARAMETER_LOGICAL_ID },
          { Ref: "AWS::NoValue" },
        ]),
      }),
      ReplicationInstanceArn: { Ref: instanceId },
    });
    expect(task).not.toHaveProperty("CdcStartTime");
    expect(source?.PostgreSqlSettings).toMatchObject({
      SlotName: `aura_historia_dms_cdc_${stage}`,
      PluginName: "test-decoding",
    });

    expect(streams).toHaveLength(1);
    expect(streams[0]).toMatchObject({
      Name: `aura-historia-cdc-${stage}`,
      ShardCount: 1,
      RetentionPeriodHours: 168,
      StreamModeDetails: { StreamMode: "PROVISIONED" },
    });
    expect(JSON.stringify(streams[0].StreamEncryption)).toContain("KMS");
  });

  test("keeps DMS private-DNS interface endpoints scoped to Kinesis and the shared application and migration secret contract", () => {
    const template = dataTemplate(stage);
    const endpoints = resourceProperties(template, "AWS::EC2::VPCEndpoint");
    const dmsEndpoints = endpoints.filter((endpoint) => {
      const policy = JSON.stringify(endpoint.PolicyDocument);
      return policy.includes("kinesis:PutRecord") || policy.includes("secretsmanager:DescribeSecret");
    });

    expect(dmsEndpoints).toHaveLength(2);
    for (const endpoint of dmsEndpoints) {
      expect(endpoint).toMatchObject({
        VpcEndpointType: "Interface",
        PrivateDnsEnabled: true,
      });
      expect(endpoint.SubnetIds).toHaveLength(2);
      expect(endpoint.SecurityGroupIds).toHaveLength(1);
    }
    expect(JSON.stringify(dmsEndpoints)).toContain("kinesis-streams");
    expect(JSON.stringify(dmsEndpoints)).toContain("secretsmanager");
    expect(JSON.stringify(dmsEndpoints)).toContain("StoragePostgresAdminCredentials");
    expect(JSON.stringify(dmsEndpoints)).toContain("StoragePostgresRuntimeCredentials");
    expect(JSON.stringify(dmsEndpoints)).toContain("StoragePostgresMigrationCredentials");
    expect(JSON.stringify(dmsEndpoints)).toContain("StoragePostgresReplicationCredentials");

    const endpointPolicies = dmsEndpoints.map((endpoint) => JSON.stringify(endpoint.PolicyDocument));
    expect(endpointPolicies.some((policy) => policy.includes("kinesis:PutRecord"))).toBe(true);
    expect(endpointPolicies.some((policy) => policy.includes("kinesis:PutRecords"))).toBe(true);
    expect(endpointPolicies.some((policy) => policy.includes("secretsmanager:GetSecretValue"))).toBe(true);
  });

  test("uses generated replication credentials and narrow endpoint-specific DMS service roles", () => {
    const template = dataTemplate(stage);
    const secrets = resourceProperties(template, "AWS::SecretsManager::Secret");
    const roles = resourceProperties(template, "AWS::IAM::Role");
    const policies = resourceProperties(template, "AWS::IAM::Policy");
    const streams = template.findResources("AWS::Kinesis::Stream");
    const [streamId] = Object.keys(streams);
    const streamArn = { "Fn::GetAtt": [streamId, "Arn"] };

    const replicationSecret = secrets.find((secret) => secret.Name === `/aura-historia/${stage}/postgres/replication`);
    const runtimeSecret = secrets.find((secret) => secret.Name === `/aura-historia/${stage}/postgres/runtime`);
    expect(replicationSecret).toBeDefined();
    expect(runtimeSecret).toBeDefined();
    expect(JSON.stringify(replicationSecret?.GenerateSecretString)).toContain("aura_replication");
    expect(JSON.stringify(replicationSecret?.GenerateSecretString)).toContain("\\\"engine\\\":\\\"postgres\\\"");
    expect(JSON.stringify(template.toJSON().Outputs ?? {})).not.toContain("aura_replication");

    const sourceSecretsRole = roles.find((role) => role.RoleName === `aura-historia-dms-source-secrets-${stage}`);
    const kinesisTargetRole = roles.find((role) => role.RoleName === `aura-historia-dms-kinesis-target-${stage}`);
    expect(sourceSecretsRole).toMatchObject({
      AssumeRolePolicyDocument: { Statement: [expect.objectContaining({ Principal: { Service: "dms.eu-central-1.amazonaws.com" } })] },
    });
    expect(kinesisTargetRole).toMatchObject({
      AssumeRolePolicyDocument: { Statement: [expect.objectContaining({ Principal: { Service: "dms.amazonaws.com" } })] },
    });
    expect(sourceSecretsRole?.AssumeRolePolicyDocument).not.toEqual(kinesisTargetRole?.AssumeRolePolicyDocument);
    const sourceSecretsPolicy = policies.find((policy) =>
      JSON.stringify(policy.Roles).includes("DmsCdcDmsSourceSecretsRole")
      && JSON.stringify(policy.PolicyDocument).includes("secretsmanager:GetSecretValue"),
    );
    expect(sourceSecretsPolicy).toBeDefined();
    expect(JSON.stringify(sourceSecretsPolicy?.PolicyDocument)).toContain("StoragePostgresReplicationCredentials");
    expect(JSON.stringify(sourceSecretsPolicy?.PolicyDocument)).not.toContain("StoragePostgresRuntimeCredentials");
    expect(JSON.stringify(sourceSecretsPolicy?.PolicyDocument)).not.toContain("\"Resource\":\"*\"");

    const targetPolicy = policies.find((policy) =>
      JSON.stringify(policy.PolicyDocument).includes("kinesis:PutRecords"),
    );
    expect(targetPolicy?.PolicyDocument).toMatchObject({
      Statement: [expect.objectContaining({
        Action: ["kinesis:DescribeStream", "kinesis:DescribeStreamSummary", "kinesis:PutRecord", "kinesis:PutRecords"],
        Resource: streamArn,
      })],
    });
    const kinesisEndpoint = resourceProperties(template, "AWS::EC2::VPCEndpoint")
      .find((endpoint) => JSON.stringify(endpoint.ServiceName).includes("kinesis-streams"));
    expect(kinesisEndpoint?.PolicyDocument).toMatchObject({
      Statement: [expect.objectContaining({
        Action: ["kinesis:DescribeStream", "kinesis:DescribeStreamSummary", "kinesis:PutRecord", "kinesis:PutRecords"],
        Resource: streamArn,
      })],
    });
    expect(JSON.stringify(policies)).toContain("secretsmanager:GetSecretValue");
  });

  test("uses the selected database, DMS PostgreSQL plugin spelling, and decoupled task LOB limit", () => {
    const template = dataTemplate(stage);
    const endpoints = resourceProperties(template, "AWS::DMS::Endpoint");
    const source = endpoints.find((endpoint) => endpoint.EndpointType === "source");

    expect(source).toMatchObject({
      DatabaseName: "aura_historia",
      EngineName: "postgres",
      PostgreSqlSettings: expect.objectContaining({
        CaptureDdls: false,
        FailTasksOnLobTruncation: true,
        PluginName: "test-decoding",
        SlotName: `aura_historia_dms_cdc_${stage}`,
      }),
    });
    expect((source?.PostgreSqlSettings as Record<string, unknown>).MaxFileSize).toBeUndefined();
  });

  test("activates only the product-listing event journal while retaining its minimal CDC payload", () => {
    const template = dataTemplate(stage);
    const [task] = resourceProperties(template, "AWS::DMS::ReplicationTask");
    const mappings = JSON.parse(task.TableMappings as string) as { rules: Record<string, unknown>[] };
    const settings = JSON.parse(task.ReplicationTaskSettings as string) as {
      TargetMetadata: Record<string, unknown>;
    };

    const includedTables = mappings.rules
      .filter((rule) => rule["rule-type"] === "selection")
      .map((rule) => (rule["object-locator"] as Record<string, string>)["table-name"])
      .sort();
    expect(includedTables).toEqual(["product_listing_events"]);

    const removedColumns = mappings.rules
      .filter((rule) => rule["rule-action"] === "remove-column")
      .map((rule) => {
        const locator = rule["object-locator"] as Record<string, string>;
        return `${locator["table-name"]}.${locator["column-name"]}`;
      });
    expect(removedColumns).toEqual(["product_listing_events.created"]);

    const decimalStrings = mappings.rules
      .filter((rule) => rule["rule-action"] === "change-data-type");
    expect(decimalStrings).toEqual([]);
    expect(settings.TargetMetadata).toMatchObject({
      BatchApplyEnabled: false,
      FullLobMode: false,
      LimitedSizeLobMode: true,
      LobMaxSize: 512,
    });
  });
});

test("ephemeral declares no DMS, Kinesis, or interface-endpoint CDC resources", () => {
  const template = dataTemplate("ephemeral");

  template.resourceCountIs("AWS::DMS::ReplicationInstance", 0);
  template.resourceCountIs("AWS::DMS::ReplicationSubnetGroup", 0);
  template.resourceCountIs("AWS::DMS::Endpoint", 0);
  template.resourceCountIs("AWS::DMS::ReplicationTask", 0);
  template.resourceCountIs("AWS::Kinesis::Stream", 0);
  template.resourceCountIs("AWS::EC2::VPCEndpoint", 0);
});
