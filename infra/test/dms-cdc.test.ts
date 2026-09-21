import * as cdk from "aws-cdk-lib";
import { Template } from "aws-cdk-lib/assertions";
import { createApplicationStacks } from "../src/application-stack";
import type { StageName } from "../src/config";

const REAL_STAGES = ["dev", "prod"] as const;

function dataTemplate(stage: StageName): Template {
  const app = new cdk.App({ analyticsReporting: false });
  return Template.fromStack(createApplicationStacks(app, { stage }).data);
}

function resourceProperties(template: Template, type: string): Record<string, unknown>[] {
  return Object.values(template.findResources(type)).map((resource) => resource.Properties as Record<string, unknown>);
}

describe.each(REAL_STAGES)("%s private DMS CDC", (stage) => {
  test("declares one private provisioned DMS instance, CDC-only task, and Kinesis target", () => {
    const template = dataTemplate(stage);
    const [instance] = resourceProperties(template, "AWS::DMS::ReplicationInstance");
    const [task] = resourceProperties(template, "AWS::DMS::ReplicationTask");
    const streams = resourceProperties(template, "AWS::Kinesis::Stream");

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
      CdcStartPosition: "now",
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

  test("uses private-DNS interface endpoints with only DMS-to-endpoint TLS ingress", () => {
    const template = dataTemplate(stage);
    const endpoints = resourceProperties(template, "AWS::EC2::VPCEndpoint");

    expect(endpoints).toHaveLength(2);
    for (const endpoint of endpoints) {
      expect(endpoint).toMatchObject({
        VpcEndpointType: "Interface",
        PrivateDnsEnabled: true,
      });
      expect(endpoint.SubnetIds).toHaveLength(2);
      expect(endpoint.SecurityGroupIds).toHaveLength(1);
    }
    expect(JSON.stringify(endpoints)).toContain("kinesis-streams");
    expect(JSON.stringify(endpoints)).toContain("secretsmanager");

    const endpointPolicies = endpoints.map((endpoint) => JSON.stringify(endpoint.PolicyDocument));
    expect(endpointPolicies.some((policy) => policy.includes("kinesis:PutRecord"))).toBe(true);
    expect(endpointPolicies.some((policy) => policy.includes("kinesis:PutRecords"))).toBe(true);
    expect(endpointPolicies.some((policy) => policy.includes("secretsmanager:GetSecretValue"))).toBe(true);
  });

  test("uses generated replication credentials and narrow DMS service roles", () => {
    const template = dataTemplate(stage);
    const secrets = resourceProperties(template, "AWS::SecretsManager::Secret");
    const roles = resourceProperties(template, "AWS::IAM::Role");
    const policies = resourceProperties(template, "AWS::IAM::Policy");

    const replicationSecret = secrets.find((secret) => secret.Name === `/aura-historia/${stage}/postgres/replication`);
    expect(replicationSecret).toBeDefined();
    expect(JSON.stringify(replicationSecret?.GenerateSecretString)).toContain("aura_replication");
    expect(JSON.stringify(replicationSecret?.GenerateSecretString)).toContain("\\\"engine\\\":\\\"postgres\\\"");
    expect(JSON.stringify(template.toJSON().Outputs ?? {})).not.toContain("aura_replication");

    expect(roles).toEqual(expect.arrayContaining([
      expect.objectContaining({ RoleName: `aura-historia-dms-source-secrets-${stage}` }),
      expect.objectContaining({ RoleName: `aura-historia-dms-kinesis-target-${stage}` }),
    ]));
    const serializedPolicies = JSON.stringify(policies);
    expect(serializedPolicies).toContain("secretsmanager:GetSecretValue");
    expect(serializedPolicies).toContain("kinesis:DescribeStreamSummary");
    expect(serializedPolicies).toContain("kinesis:PutRecord");
    expect(serializedPolicies).toContain("kinesis:PutRecords");
  });

  test("keeps only the selected routing columns and emits exact decimal strings for bigint wakeups", () => {
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
    expect(includedTables).toEqual([
      "notification_deliveries",
      "product_listing_events",
      "product_listing_raw_revisions",
      "search_filter_matches",
      "search_filters",
    ]);

    const removedColumns = mappings.rules
      .filter((rule) => rule["rule-action"] === "remove-column")
      .map((rule) => {
        const locator = rule["object-locator"] as Record<string, string>;
        return `${locator["table-name"]}.${locator["column-name"]}`;
      });
    expect(removedColumns).toEqual(expect.arrayContaining([
      "product_listing_raw_revisions.source_payload",
      "product_listing_raw_revisions.raw_values",
      "product_listing_raw_revisions.normalization_context",
      "product_listing_raw_revisions.provenance",
      "search_filters.search",
      "notification_deliveries.target_key",
    ]));

    const decimalStrings = mappings.rules
      .filter((rule) => rule["rule-action"] === "change-data-type")
      .map((rule) => {
        const locator = rule["object-locator"] as Record<string, string>;
        return `${locator["table-name"]}.${locator["column-name"]}`;
      })
      .sort();
    expect(decimalStrings).toEqual([
      "product_listing_raw_revisions.revision",
      "search_filters.version",
    ]);
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
