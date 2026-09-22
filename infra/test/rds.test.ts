import * as cdk from "aws-cdk-lib";
import { Template } from "aws-cdk-lib/assertions";
import { createHash } from "node:crypto";
import { readFileSync } from "node:fs";
import * as path from "node:path";
import { createApplicationStacks } from "../src/application-stack";
import type { StageName } from "../src/config";

const REAL_STAGES = ["dev", "prod"] as const;
const PRODUCTION_POSTGRES_TLS_ROOT_CERTIFICATE = "/opt/aura-historia/rds-ca/global-bundle.pem";
const EPHEMERAL_POSTGRES_TLS_ROOT_CERTIFICATE = "/var/task/aura-historia/test-postgres-ca.pem";
const PUBLIC_RDS_CA_ASSET = path.join(
  __dirname,
  "../assets/rds-ca-layer/aura-historia/rds-ca/global-bundle.pem",
);
const PUBLIC_RDS_CA_ASSET_SHA256 = "e5bb2084ccf45087bda1c9bffdea0eb15ee67f0b91646106e466714f9de3c7e3";

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

  test("keeps runtime and private migration PostgreSQL secrets separately scoped", () => {
    const stacks = createStacks(stage);
    const compute = Template.fromStack(stacks.compute);
    const initialization = Template.fromStack(stacks.initialization!);
    const functions = [
      ...Object.values(compute.findResources("AWS::Lambda::Function")),
      ...Object.values(initialization.findResources("AWS::Lambda::Function")),
    ].filter((resource) => resource.Properties.Environment?.Variables?.POSTGRES_HOST !== undefined);
    const migration = functions.find((resource) =>
      resource.Properties.FunctionName === `database-migration-lambda-${stage}`,
    );
    const runtimeFunctions = functions.filter((resource) =>
      resource.Properties.Environment.Variables.POSTGRES_SECRET_ARN !== undefined,
    );

    expect(functions).toHaveLength(8);
    expect(runtimeFunctions).toHaveLength(7);
    expect(migration).toBeDefined();
    expect(functions.find((resource) =>
      resource.Properties.FunctionName === `product-listing-normalization-lambda-${stage}`,
    )).toBeDefined();
    expect(Object.values(initialization.findResources("AWS::Lambda::Function"))).toHaveLength(1);
    initialization.resourceCountIs("AWS::Lambda::EventSourceMapping", 0);
    initialization.resourceCountIs("AWS::Events::Rule", 0);
    expect(JSON.stringify(compute.toJSON())).not.toContain(`database-migration-lambda-${stage}`);
    expect(JSON.stringify(compute.toJSON())).toContain(`fxrate-lambda-${stage}`);
    const runtimeSecretArn = runtimeFunctions[0].Properties.Environment.Variables.POSTGRES_SECRET_ARN;
    expect(runtimeSecretArn).toBeDefined();
    for (const functionResource of runtimeFunctions) {
      const environment = functionResource.Properties.Environment.Variables;
      expect(JSON.stringify(environment.POSTGRES_HOST)).not.toContain(`/postgres/${stage}/host`);
      expect(environment.POSTGRES_SECRET_ARN).toEqual(runtimeSecretArn);
      expect(environment.POSTGRES_USERNAME).toBeUndefined();
      expect(environment.POSTGRES_PASSWORD).toBeUndefined();
      expect(JSON.stringify(environment)).not.toContain("resolve:secretsmanager:");
      expect(environment.POSTGRES_MAX_CONNECTIONS).toBe("1");
      expect(environment.POSTGRES_TLS_ROOT_CERT).toBe(PRODUCTION_POSTGRES_TLS_ROOT_CERTIFICATE);
      expect(functionResource.Properties.Layers).toHaveLength(1);
      expect(JSON.stringify(functionResource.Properties.Layers)).toContain("PostgresTlsRootCertificateLayer");
    }

    const migrationEnvironment = migration!.Properties.Environment.Variables;
    expect(migration!.Properties).toMatchObject({
      Runtime: "provided.al2023",
      Architectures: ["x86_64"],
      Timeout: 840,
    });
    expect(migrationEnvironment.POSTGRES_SECRET_ARN).toBeUndefined();
    expect(migrationEnvironment.POSTGRES_USERNAME).toBeUndefined();
    expect(migrationEnvironment.POSTGRES_PASSWORD).toBeUndefined();
    expect(migrationEnvironment.POSTGRES_MAX_CONNECTIONS).toBe("1");
    expect(migrationEnvironment.POSTGRES_TLS_ROOT_CERT).toBe(PRODUCTION_POSTGRES_TLS_ROOT_CERTIFICATE);
    expect(Object.keys(migrationEnvironment).filter((name) => name.endsWith("_SECRET_ARN"))).toEqual([
      "POSTGRES_ADMIN_SECRET_ARN",
      "POSTGRES_MIGRATION_SECRET_ARN",
      "POSTGRES_REPLICATION_SECRET_ARN",
      "POSTGRES_RUNTIME_SECRET_ARN",
    ]);
    expect(migration!.Properties.Layers).toHaveLength(1);

    const secretReadStatements = [
      ...Object.values(compute.findResources("AWS::IAM::Policy")),
      ...Object.values(initialization.findResources("AWS::IAM::Policy")),
    ]
      .flatMap((policy) => policy.Properties.PolicyDocument.Statement)
      .filter((statement) => JSON.stringify(statement.Action).includes("secretsmanager:GetSecretValue"));
    const runtimeSecretReadStatements = secretReadStatements.filter((statement) =>
      JSON.stringify(statement.Resource) === JSON.stringify(runtimeSecretArn),
    );
    const migrationSecretReadStatement = secretReadStatements.find((statement) =>
      Array.isArray(statement.Resource) && statement.Resource.length === 4,
    );
    expect(runtimeSecretReadStatements).toHaveLength(7);
    expect(migrationSecretReadStatement).toMatchObject({
      Action: "secretsmanager:GetSecretValue",
      Effect: "Allow",
    });
    expect(migrationSecretReadStatement?.Resource).toHaveLength(4);
    expect(JSON.stringify(migrationSecretReadStatement?.Resource)).toContain("PostgresAdminCredentials");
    expect(JSON.stringify(migrationSecretReadStatement?.Resource)).toContain("PostgresRuntimeCredentials");
    expect(JSON.stringify(migrationSecretReadStatement?.Resource)).toContain("PostgresMigrationCredentials");
    expect(JSON.stringify(migrationSecretReadStatement?.Resource)).toContain("PostgresReplicationCredentials");

    const layers = [
      ...Object.values(compute.findResources("AWS::Lambda::LayerVersion")),
      ...Object.values(initialization.findResources("AWS::Lambda::LayerVersion")),
    ];
    expect(layers).toHaveLength(2);
    for (const layer of layers) {
      expect(layer.Properties).toMatchObject({
        CompatibleRuntimes: ["provided.al2023"],
        Description: "Public AWS RDS root certificate bundle for PostgreSQL Lambdas",
      });
    }
  });

  test("routes runtime credential retrieval through the shared private DMS Secrets Manager endpoint", () => {
    const stacks = createStacks(stage);
    const network = Template.fromStack(stacks.network!);
    const data = Template.fromStack(stacks.data);
    const compute = Template.fromStack(stacks.compute);
    const secrets = data.findResources("AWS::SecretsManager::Secret");
    const runtimeSecret = Object.entries(secrets)
      .find(([, secret]) => secret.Properties.Name === `/aura-historia/${stage}/postgres/runtime`);
    const replicationSecret = Object.entries(secrets)
      .find(([, secret]) => secret.Properties.Name === `/aura-historia/${stage}/postgres/replication`);
    const adminSecret = Object.entries(secrets)
      .find(([, secret]) => secret.Properties.Name === `/aura-historia/${stage}/postgres/admin`);
    const migrationSecret = Object.entries(secrets)
      .find(([, secret]) => secret.Properties.Name === `/aura-historia/${stage}/postgres/migrator`);
    const secretsManagerEndpoints = Object.values(data.findResources("AWS::EC2::VPCEndpoint"))
      .filter((endpoint) => JSON.stringify(endpoint.Properties.ServiceName).includes("secretsmanager"));
    const groups = Object.entries(network.findResources("AWS::EC2::SecurityGroup"));
    const applicationSecurityGroup = groups
      .find(([, group]) => group.Properties.GroupDescription === "Backend application workloads in private application subnets");
    const dmsEndpointSecurityGroup = groups
      .find(([, group]) => group.Properties.GroupDescription === "DMS interface endpoint boundary");

    expect(runtimeSecret).toBeDefined();
    expect(replicationSecret).toBeDefined();
    expect(adminSecret).toBeDefined();
    expect(migrationSecret).toBeDefined();
    expect(secretsManagerEndpoints).toHaveLength(1);
    expect(applicationSecurityGroup).toBeDefined();
    expect(dmsEndpointSecurityGroup).toBeDefined();

    const [runtimeSecretId] = runtimeSecret!;
    const [replicationSecretId] = replicationSecret!;
    const [adminSecretId] = adminSecret!;
    const [migrationSecretId] = migrationSecret!;
    const [applicationSecurityGroupId] = applicationSecurityGroup!;
    const [dmsEndpointSecurityGroupId] = dmsEndpointSecurityGroup!;
    const [secretsManagerEndpoint] = secretsManagerEndpoints;
    const endpointPolicy = secretsManagerEndpoint.Properties.PolicyDocument;
    const policyStatements = endpointPolicy.Statement as {
      readonly Action: unknown;
      readonly Resource: unknown;
    }[];
    const getSecretValueStatement = policyStatements
      .find((statement) => JSON.stringify(statement.Action).includes("secretsmanager:GetSecretValue"));
    const describeSecretStatement = policyStatements
      .find((statement) => JSON.stringify(statement.Action).includes("secretsmanager:DescribeSecret"));
    const policyResources = policyStatements.flatMap((statement) =>
      Array.isArray(statement.Resource) ? statement.Resource : [statement.Resource],
    );

    expect(secretsManagerEndpoint.Properties).toMatchObject({
      PrivateDnsEnabled: true,
      VpcEndpointType: "Interface",
    });
    expect(JSON.stringify(secretsManagerEndpoint.Properties.ServiceName)).toContain("secretsmanager");
    expect(JSON.stringify(secretsManagerEndpoint.Properties.SecurityGroupIds)).toContain(dmsEndpointSecurityGroupId);
    expect(getSecretValueStatement).toMatchObject({
      Action: "secretsmanager:GetSecretValue",
      Effect: "Allow",
      Principal: { AWS: "*" },
      Resource: [
        { Ref: adminSecretId },
        { Ref: runtimeSecretId },
        { Ref: migrationSecretId },
        { Ref: replicationSecretId },
      ],
    });
    expect(describeSecretStatement).toMatchObject({
      Action: "secretsmanager:DescribeSecret",
      Effect: "Allow",
      Principal: { AWS: "*" },
      Resource: { Ref: replicationSecretId },
    });
    expect(policyResources).not.toContain("*");
    expect(JSON.stringify(policyResources)).toContain(adminSecretId);
    expect(JSON.stringify(policyResources)).toContain(migrationSecretId);

    const applicationEndpointEgress = Object.values(network.findResources("AWS::EC2::SecurityGroupEgress"))
      .find((rule) => JSON.stringify(rule.Properties.GroupId).includes(applicationSecurityGroupId)
        && JSON.stringify(rule.Properties.DestinationSecurityGroupId).includes(dmsEndpointSecurityGroupId));
    const endpointApplicationIngress = Object.values(network.findResources("AWS::EC2::SecurityGroupIngress"))
      .find((rule) => JSON.stringify(rule.Properties.GroupId).includes(dmsEndpointSecurityGroupId)
        && JSON.stringify(rule.Properties.SourceSecurityGroupId).includes(applicationSecurityGroupId));
    const endpointIngress = Object.values(network.findResources("AWS::EC2::SecurityGroupIngress"))
      .filter((rule) => JSON.stringify(rule.Properties.GroupId).includes(dmsEndpointSecurityGroupId));
    const runtimeLambdaSecurityGroup = Object.values(compute.findResources("AWS::Lambda::Function"))
      .find((resource) => resource.Properties.FunctionName === `aura-historia-api-${stage}`)
      ?.Properties.VpcConfig.SecurityGroupIds[0];

    expect(applicationEndpointEgress?.Properties).toMatchObject({
      IpProtocol: "tcp",
      FromPort: 443,
      ToPort: 443,
    });
    expect(endpointApplicationIngress?.Properties).toMatchObject({
      IpProtocol: "tcp",
      FromPort: 443,
      ToPort: 443,
    });
    expect(endpointIngress).toHaveLength(3);
    expect(endpointIngress.every((rule) => rule.Properties.SourceSecurityGroupId !== undefined
      && rule.Properties.CidrIp === undefined)).toBe(true);
    expect(JSON.stringify(runtimeLambdaSecurityGroup)).toContain(applicationSecurityGroupId);
  });

  test("uses a pinned public RDS CA asset without private material", () => {
    const publicBundle = readFileSync(PUBLIC_RDS_CA_ASSET);

    expect(publicBundle.toString("utf8")).toContain("-----BEGIN CERTIFICATE-----");
    expect(publicBundle.toString("utf8")).not.toMatch(/PRIVATE KEY|ENCRYPTED/);
    expect(createHash("sha256").update(publicBundle).digest("hex")).toBe(PUBLIC_RDS_CA_ASSET_SHA256);
  });
});

test("ephemeral packages PostgreSQL Lambdas for the generated test CA without a production layer", () => {
  const stacks = createStacks("ephemeral");
  const compute = Template.fromStack(stacks.compute);
  const functions = Object.values(compute.findResources("AWS::Lambda::Function"))
    .filter((resource) => resource.Properties.Environment?.Variables?.POSTGRES_HOST !== undefined);

  expect(functions).toHaveLength(6);
  expect(functions.find((resource) =>
    resource.Properties.FunctionName === "product-listing-normalization-lambda-ephemeral",
  )).toBeDefined();
  for (const functionResource of functions) {
    const environment = functionResource.Properties.Environment.Variables;
    expect(environment.POSTGRES_TLS_ROOT_CERT).toBe(EPHEMERAL_POSTGRES_TLS_ROOT_CERTIFICATE);
    expect(environment.POSTGRES_SECRET_ARN).toBeUndefined();
    expect(environment.POSTGRES_USERNAME).toBe("postgres");
    expect(environment.POSTGRES_PASSWORD).toBe("postgres");
    expect(functionResource.Properties.Layers).toBeUndefined();
  }
  expect(Object.values(compute.findResources("AWS::Lambda::LayerVersion"))).toHaveLength(0);
  expect(JSON.stringify(compute.toJSON())).not.toContain(PRODUCTION_POSTGRES_TLS_ROOT_CERTIFICATE);
});

test("ephemeral creates no RDS or generated database credentials", () => {
  const stacks = createStacks("ephemeral");
  const template = Template.fromStack(stacks.data);

  template.resourceCountIs("AWS::RDS::DBInstance", 0);
  template.resourceCountIs("AWS::RDS::DBSubnetGroup", 0);
  template.resourceCountIs("AWS::RDS::DBParameterGroup", 0);
  template.resourceCountIs("AWS::SecretsManager::Secret", 0);
});
