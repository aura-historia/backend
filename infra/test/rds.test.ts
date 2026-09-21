import * as cdk from "aws-cdk-lib";
import { Template } from "aws-cdk-lib/assertions";
import { createApplicationStacks } from "../src/application-stack";
import type { StageName } from "../src/config";

const REAL_STAGES = ["dev", "prod"] as const;

interface RdsStageExpectation {
  readonly instanceClass: string;
  readonly allocatedStorage: number;
  readonly maxAllocatedStorage: number;
  readonly backupRetention: number;
}

const EXPECTATIONS: Record<"dev" | "prod", RdsStageExpectation> = {
  dev: { instanceClass: "db.t4g.small", allocatedStorage: 30, maxAllocatedStorage: 60, backupRetention: 7 },
  prod: { instanceClass: "db.t4g.medium", allocatedStorage: 50, maxAllocatedStorage: 100, backupRetention: 14 },
};

function createStacks(stage: StageName) {
  const app = new cdk.App({ analyticsReporting: false });
  return createApplicationStacks(app, { stage, stackNamePrefix: `application-${stage}` });
}

describe.each(REAL_STAGES)("%s RDS PostgreSQL foundation", (stage) => {
  test("uses one private encrypted single-AZ PostgreSQL instance with deliberate capacity and recovery settings", () => {
    const stacks = createStacks(stage);
    const template = Template.fromStack(stacks.data);
    const expected = EXPECTATIONS[stage];
    const instances = Object.values(template.findResources("AWS::RDS::DBInstance"));

    expect(instances).toHaveLength(1);
    template.resourceCountIs("AWS::RDS::DBSubnetGroup", 1);
    template.resourceCountIs("AWS::RDS::DBParameterGroup", 1);

    const instance = instances[0];
    expect(instance.Properties).toMatchObject({
      DBInstanceClass: expected.instanceClass,
      Engine: "postgres",
      EngineVersion: "16.13",
      DBName: "aura_historia",
      AllocatedStorage: String(expected.allocatedStorage),
      MaxAllocatedStorage: expected.maxAllocatedStorage,
      StorageType: "gp3",
      StorageEncrypted: true,
      PubliclyAccessible: false,
      MultiAZ: false,
      BackupRetentionPeriod: expected.backupRetention,
      PreferredBackupWindow: "02:00-02:30",
      PreferredMaintenanceWindow: "sun:03:00-sun:03:30",
      AutoMinorVersionUpgrade: true,
      CopyTagsToSnapshot: true,
    });
    expect(instance.Properties.DBSubnetGroupName).toBeDefined();
    expect(instance.Properties.VPCSecurityGroups).toHaveLength(1);
    expect(instance.Properties.EnableCloudwatchLogsExports).toContain("postgresql");
    expect(instance.Properties.DeleteAutomatedBackups).toBe(stage === "dev");
    expect(instance.Properties.DeletionProtection).toBe(stage === "prod");

    if (stage === "prod") {
      expect(instance.DeletionPolicy).toBe("Retain");
      expect(instance.UpdateReplacePolicy).toBe("Retain");
    } else {
      expect(instance.DeletionPolicy).toBe("Delete");
      expect(instance.UpdateReplacePolicy).toBe("Delete");
    }
  });

  test("sets TLS and logical replication policy for the private DMS CDC task", () => {
    const stacks = createStacks(stage);
    const template = Template.fromStack(stacks.data);
    const parameterGroups = Object.values(template.findResources("AWS::RDS::DBParameterGroup"));

    expect(parameterGroups).toHaveLength(1);

    expect(parameterGroups[0].Properties.Parameters).toMatchObject({
      "rds.force_ssl": "1",
      "rds.logical_replication": "1",
      max_replication_slots: "5",
      max_wal_senders: "5",
      max_slot_wal_keep_size: "10240",
    });
    template.resourceCountIs("AWS::RDS::DBProxy", 0);
    template.resourceCountIs("AWS::RDS::DBCluster", 0);
    expect(Object.keys(template.findResources("AWS::DMS::ReplicationInstance"))).toHaveLength(1);
    expect(Object.keys(template.findResources("AWS::DMS::Endpoint"))).toHaveLength(2);
    expect(Object.keys(template.findResources("AWS::DMS::ReplicationTask"))).toHaveLength(1);
  });

  test("creates generated role credentials and keeps credential values out of outputs", () => {
    const stacks = createStacks(stage);
    const template = Template.fromStack(stacks.data);
    const secrets = Object.values(template.findResources("AWS::SecretsManager::Secret"));
    const outputs = template.toJSON().Outputs ?? {};

    expect(secrets).toHaveLength(4);
    expect(secrets.every((secret) => secret.Properties.GenerateSecretString !== undefined)).toBe(true);
    expect(JSON.stringify(outputs)).not.toContain("Secret");
    expect(JSON.stringify(outputs)).not.toContain("aura_runtime");
    expect(JSON.stringify(outputs)).not.toContain("aura_migrator");
    expect(JSON.stringify(outputs)).not.toContain("aura_replication");
  });

  test("passes the RDS endpoint and runtime generated-secret references to PostgreSQL Lambdas", () => {
    const stacks = createStacks(stage);
    const compute = Template.fromStack(stacks.compute);
    const functions = Object.values(compute.findResources("AWS::Lambda::Function"))
      .filter((resource) => resource.Properties.Environment?.Variables?.POSTGRES_HOST !== undefined);

    expect(functions).toHaveLength(4);
    for (const functionResource of functions) {
      const environment = functionResource.Properties.Environment.Variables;
      expect(JSON.stringify(environment.POSTGRES_HOST)).not.toContain(`/postgres/${stage}/host`);
      expect(JSON.stringify(environment.POSTGRES_USERNAME)).toContain("resolve:secretsmanager:");
      expect(JSON.stringify(environment.POSTGRES_PASSWORD)).toContain("resolve:secretsmanager:");
      expect(environment.POSTGRES_MAX_CONNECTIONS).toBe("1");
      expect(environment.POSTGRES_TLS_ROOT_CERT).toBe("/etc/pki/ca-trust/extracted/pem/tls-ca-bundle.pem");
    }
  });
});

test("ephemeral creates no RDS or generated database credentials", () => {
  const stacks = createStacks("ephemeral");
  const template = Template.fromStack(stacks.data);

  template.resourceCountIs("AWS::RDS::DBInstance", 0);
  template.resourceCountIs("AWS::RDS::DBSubnetGroup", 0);
  template.resourceCountIs("AWS::RDS::DBParameterGroup", 0);
  template.resourceCountIs("AWS::SecretsManager::Secret", 0);
});
