import * as cdk from "aws-cdk-lib";
import { Template } from "aws-cdk-lib/assertions";
import { execFileSync, spawnSync } from "node:child_process";
import * as fs from "node:fs";
import * as os from "node:os";
import * as path from "node:path";
import {
  ApplicationComputeStack,
  ApplicationDataStack,
  ApplicationEphemeralStack,
  createApplicationStacks,
} from "../src/application-stack";
import { CLOUDFORMATION_STAGING_BUCKET_NAME, STAGES, type StageName } from "../src/config";
import { DATABASE_LAMBDA_KEYS, lambdaFunctionName, type DatabaseLambdaKey } from "../src/constructs/lambdas";
import type { LambdaEgressOutput } from "../src/constructs/lambda-egress";
import type { PostgresLambdaConfig } from "../src/postgres-lambda-config";

const SAFE_ENV = {
  PATH: [path.dirname(process.execPath), "/usr/local/bin", "/usr/bin", "/bin"].join(path.delimiter),
  AWS_CONFIG_FILE: "/dev/null",
  AWS_SHARED_CREDENTIALS_FILE: "/dev/null",
  AWS_EC2_METADATA_DISABLED: "true",
};
const ENVIRONMENT = { account: "123456789012", region: "eu-central-1" };
const DB_KEYS: readonly DatabaseLambdaKey[] = ["postConfirmation", "shopify", "stripe", "fxRateSync"];
const ENI_ACTIONS = [
  "ec2:CreateNetworkInterface", "ec2:DeleteNetworkInterface", "ec2:DescribeNetworkInterfaces",
  "ec2:DescribeSubnets", "ec2:DetachNetworkInterface",
  "ec2:AssignPrivateIpAddresses", "ec2:UnassignPrivateIpAddresses",
];
const temporaryDirectories: string[] = [];
let publicCa: string;
let previousUmask: number;

type Resource = { Type: string; Properties: Record<string, any>; DependsOn?: string[]; [key: string]: any };
type StackTemplate = { Resources: Record<string, Resource>; Outputs?: Record<string, any>; Parameters?: Record<string, any> };

function temporaryDirectory(): string {
  const directory = fs.mkdtempSync(path.join(os.tmpdir(), "postgres-lambda-test-"));
  temporaryDirectories.push(directory);
  fs.chmodSync(directory, 0o755);
  return directory;
}

function appFixture(): cdk.App {
  return new cdk.App({ outdir: temporaryDirectory(), analyticsReporting: false, autoSynth: false });
}

function configFixture(stage: "dev" | "prod" = "dev"): PostgresLambdaConfig {
  const caAssetDirectory = temporaryDirectory();
  const caDirectory = path.join(caAssetDirectory, "postgres-ca");
  fs.mkdirSync(caDirectory);
  fs.chmodSync(caDirectory, 0o755);
  const caFile = path.join(caDirectory, "root.pem");
  fs.writeFileSync(caFile, publicCa);
  fs.chmodSync(caFile, 0o644);
  return {
    stage,
    environment: { ...ENVIRONMENT },
    network: {
      ipProtocol: "IPV4",
      vpcCidr: "10.42.0.0/16",
      availabilityZones: [
        { availabilityZone: "eu-central-1a", publicSubnetCidr: "10.42.0.0/24", privateSubnetCidr: "10.42.16.0/24" },
        { availabilityZone: "eu-central-1b", publicSubnetCidr: "10.42.1.0/24", privateSubnetCidr: "10.42.17.0/24" },
      ],
      natTopology: stage === "prod" ? { mode: "PER_AZ" } : { mode: "SINGLE", availabilityZone: "eu-central-1b" },
      // Syntax fixture only. No connection to this public IPv4 address is made.
      database: { destinationCidr: "8.8.8.8/32", port: 6432 },
      httpsPolicy: { mode: "PUBLIC_IPV4" },
    },
    databaseHostname: "postgres.example.test",
    caAssetDirectory,
    reservedConcurrency: { postConfirmation: 1, shopify: 3, stripe: 4, fxRateSync: 2 },
    lambdaConnectionBudget: 20,
  };
}

function resources(template: StackTemplate, type: string): Record<string, Resource> {
  return Object.fromEntries(Object.entries(template.Resources).filter(([, resource]) => resource.Type === type));
}

function only(entries: Record<string, Resource>): [string, Resource] {
  expect(Object.keys(entries)).toHaveLength(1);
  return Object.entries(entries)[0];
}

function namedFunction(template: StackTemplate, name: string): [string, Resource] {
  return only(Object.fromEntries(Object.entries(resources(template, "AWS::Lambda::Function"))
    .filter(([, resource]) => resource.Properties.FunctionName === name)));
}

function synthesize(stage: StageName, postgresLambda?: PostgresLambdaConfig) {
  const app = appFixture();
  const stacks = createApplicationStacks(app, { stage, env: ENVIRONMENT, postgresLambda });
  const assembly = app.synth();
  const data = Template.fromStack(stacks.data).toJSON() as StackTemplate;
  const compute = Template.fromStack(stacks.compute).toJSON() as StackTemplate;
  const templates = assembly.stacks.map((stack) => stack.template as StackTemplate);
  expect(assembly.manifest.missing ?? []).toEqual([]);
  expect(JSON.stringify(templates)).not.toContain("Fn::GetAZs");
  expect(stacks.compute.dependencies).toContain(stacks.data);
  expect(stacks.data.dependencies).not.toContain(stacks.compute);
  expect(assembly.getStackArtifact(stacks.compute.artifactId).dependencies.map((entry) => entry.id))
    .toContain(stacks.data.artifactId);
  return { app, stacks, assembly, data, compute, templates };
}

function exportedValue(data: StackTemplate, reference: any): any {
  expect(Object.keys(reference)).toEqual(["Fn::ImportValue"]);
  const outputs = Object.values(data.Outputs ?? {}).filter((output) => output.Export?.Name === reference["Fn::ImportValue"]);
  expect(outputs).toHaveLength(1);
  return outputs[0].Value;
}

function expectOff(templates: StackTemplate[], stage: StageName): void {
  for (const template of templates) {
    expect(Object.values(template.Resources).filter((resource) => resource.Type.startsWith("AWS::EC2::"))).toEqual([]);
    expect(resources(template, "AWS::Lambda::LayerVersion")).toEqual({});
    for (const resource of Object.values(resources(template, "AWS::Lambda::Function"))) {
      expect(resource.Properties.VpcConfig).toBeUndefined();
      expect(resource.Properties.Layers).toBeUndefined();
      expect(resource.Properties.ReservedConcurrentExecutions).toBeUndefined();
      const environment = resource.Properties.Environment?.Variables;
      if (environment?.POSTGRES_HOST !== undefined) {
        expect(environment.STAGE).toBe(stage);
        expect(environment.POSTGRES_MAX_CONNECTIONS).toBe("2");
        expect(environment.POSTGRES_SSL_MODE).toBe(stage === "ephemeral" ? "disable" : "verify-full");
        expect(environment.POSTGRES_SSL_ROOT_CERT).toBe(stage === "ephemeral"
          ? undefined : `{{resolve:ssm:/postgres/${stage}/ssl-root-cert-path}}`);
      }
    }
    for (const mapping of Object.values(resources(template, "AWS::Lambda::EventSourceMapping"))) {
      expect(mapping.Properties.ScalingConfig).toBeUndefined();
    }
    expect(JSON.stringify(template)).not.toContain("lambda:SourceFunctionArn");
  }
}

beforeAll(() => {
  // Disposable local signing key goes straight to /dev/null; only public PEM survives.
  publicCa = execFileSync("openssl", [
    "req", "-new", "-x509", "-newkey", "ec", "-pkeyopt", "ec_paramgen_curve:prime256v1",
    "-nodes", "-keyout", "/dev/null", "-subj", "/CN=Synthetic PostgreSQL Lambda test CA",
    "-days", "3650", "-addext", "basicConstraints=critical,CA:TRUE",
    "-addext", "keyUsage=critical,keyCertSign,cRLSign",
  ], { env: SAFE_ENV, encoding: "utf8", timeout: 10000, stdio: ["pipe", "pipe", "pipe"] });
});

beforeEach(() => {
  // PostgresCa requires Lambda-readable staging modes, independent of the invoking shell.
  previousUmask = process.umask(0o022);
});

afterEach(() => {
  try {
    for (const directory of temporaryDirectories.splice(0)) fs.rmSync(directory, { recursive: true, force: true });
  } finally {
    process.umask(previousUmask);
  }
});

describe("PostgreSQL Lambda application integration (offline)", () => {
  test.each(["dev", "prod"] as const)("%s attaches exactly four catalog DB functions to private subnets and the sole dedicated SG", (stage) => {
    const config = configFixture(stage);
    const { stacks, data, compute, templates } = synthesize(stage, config);
    expect([...DATABASE_LAMBDA_KEYS].sort()).toEqual([...DB_KEYS].sort());
    expect(stacks.data.lambdaEgress).toBeDefined();
    expect(stacks.data.lambdaEgress!.stage).toBe(stage);
    expect(stacks.data.lambdaEgress!.environment).toEqual(ENVIRONMENT);
    const [vpcId] = only(resources(data, "AWS::EC2::VPC"));
    const [sgId, sg] = only(resources(data, "AWS::EC2::SecurityGroup"));
    expect(sg.Properties.VpcId).toEqual({ Ref: vpcId });
    expect(sg.Properties.SecurityGroupIngress ?? []).toEqual([]);
    expect(sg.Properties.SecurityGroupEgress).toEqual(expect.arrayContaining([
      expect.objectContaining({ CidrIp: "8.8.8.8/32", IpProtocol: "tcp", FromPort: 6432, ToPort: 6432 }),
      expect.objectContaining({ CidrIp: "0.0.0.0/0", IpProtocol: "tcp", FromPort: 443, ToPort: 443 }),
    ]));
    expect(sg.Properties.SecurityGroupEgress).toHaveLength(2);
    const subnets = resources(data, "AWS::EC2::Subnet");
    const privateIds = config.network.availabilityZones.map((layout) => only(Object.fromEntries(Object.entries(subnets)
      .filter(([, subnet]) => subnet.Properties.CidrBlock === layout.privateSubnetCidr)))[0]);
    const [layerId] = only(resources(compute, "AWS::Lambda::LayerVersion"));
    const attached = Object.entries(resources(compute, "AWS::Lambda::Function"))
      .filter(([, resource]) => resource.Properties.VpcConfig !== undefined);
    expect(attached.map(([, resource]) => resource.Properties.FunctionName).sort())
      .toEqual(DB_KEYS.map((key) => lambdaFunctionName(key, stage)).sort());
    for (const key of DB_KEYS) {
      const [, fn] = namedFunction(compute, lambdaFunctionName(key, stage));
      const vpc = fn.Properties.VpcConfig;
      expect(vpc.SubnetIds.map((reference: any) => exportedValue(data, reference)))
        .toEqual(privateIds.map((id) => ({ Ref: id })));
      expect(vpc.SecurityGroupIds.map((reference: any) => exportedValue(data, reference)))
        .toEqual([{ "Fn::GetAtt": [sgId, "GroupId"] }]);
      expect(vpc.Ipv6AllowedForDualStack ?? false).toBe(false);
      expect(fn.Properties.Layers).toEqual([{ Ref: layerId }]);
      expect(fn.Properties.ReservedConcurrentExecutions).toBe(config.reservedConcurrency[key]);
      expect(fn.Properties.Environment.Variables).toEqual(expect.objectContaining({
        STAGE: stage, POSTGRES_HOST: config.databaseHostname, POSTGRES_PORT: "6432",
        POSTGRES_MAX_CONNECTIONS: "2", POSTGRES_SSL_MODE: "verify-full", POSTGRES_SSL_ROOT_CERT: "/opt/postgres-ca/root.pem",
      }));
      expect(fn.Properties.Environment.Variables.PGSSLMODE).toBeUndefined();
    }
    const excluded = templates.flatMap((template) => Object.values(resources(template, "AWS::Lambda::Function")))
      .filter((resource) => !DB_KEYS.some((key) => resource.Properties.FunctionName === lambdaFunctionName(key, stage)));
    expect(excluded.some((resource) => resource.Properties.FunctionName === `cloudwatch-log-retention-lambda-${stage}`)).toBe(true);
    expect(excluded.filter((resource) => resource.Properties.Code.ZipFile !== undefined).length).toBeGreaterThan(0);
    for (const resource of excluded) {
      expect(resource.Properties.VpcConfig).toBeUndefined();
      expect(resource.Properties.Layers).toBeUndefined();
      expect(resource.Properties.ReservedConcurrentExecutions).toBeUndefined();
      expect(resource.Properties.Environment?.Variables?.POSTGRES_HOST).toBeUndefined();
    }
    const nats = stacks.data.lambdaEgress!.natGateways;
    expect(nats.map((nat) => nat.availabilityZone)).toEqual(stage === "prod" ? ["eu-central-1a", "eu-central-1b"] : ["eu-central-1b"]);
    expect(Object.keys(resources(data, "AWS::EC2::EIP"))).toHaveLength(nats.length);
    expect(Object.keys(data.Outputs!).filter((id) => id.startsWith("LambdaNatIpv4"))).toHaveLength(nats.length);
    for (const nat of nats) {
      const outputId = stacks.data.getLogicalId(stacks.data.node.findChild(`LambdaNatIpv4${nat.availabilityZone}`) as cdk.CfnOutput);
      const value = data.Outputs![outputId].Value;
      expect(value).toEqual(stacks.data.resolve(nat.publicIpv4));
      expect(resources(data, "AWS::EC2::EIP")[value.Ref]).toBeDefined();
    }
  });

  test.each(["dev", "prod"] as const)("%s keeps function/role/code/queue IDs stable against default mode", (stage) => {
    const baseline = synthesize(stage);
    const enabled = synthesize(stage, configFixture(stage));
    for (const [before, after] of [[baseline.data, enabled.data], [baseline.compute, enabled.compute]]) {
      for (const type of ["AWS::Lambda::Function", "AWS::IAM::Role", "AWS::SQS::Queue", "AWS::Lambda::EventSourceMapping"]) {
        expect(Object.keys(resources(after, type)).sort()).toEqual(Object.keys(resources(before, type)).sort());
      }
      expect(resources(after, "AWS::SQS::Queue")).toEqual(resources(before, "AWS::SQS::Queue"));
      for (const [id, resource] of Object.entries(resources(before, "AWS::Lambda::Function"))) {
        for (const property of ["FunctionName", "Role", "Code", "Runtime", "Architectures", "Handler"]) {
          expect(after.Resources[id].Properties[property]).toEqual(resource.Properties[property]);
        }
        const oldEnv = resource.Properties.Environment?.Variables;
        if (oldEnv?.POSTGRES_HOST !== undefined) {
          for (const property of ["POSTGRES_DATABASE", "POSTGRES_USERNAME", "POSTGRES_PASSWORD"]) {
            expect(after.Resources[id].Properties.Environment.Variables[property]).toEqual(oldEnv[property]);
          }
        }
      }
      for (const [id, role] of Object.entries(resources(before, "AWS::IAM::Role"))) {
        expect(after.Resources[id].Properties.RoleName).toEqual(role.Properties.RoleName);
        expect(after.Resources[id].Properties.AssumeRolePolicyDocument).toEqual(role.Properties.AssumeRolePolicyDocument);
      }
    }
    expect(Template.fromStack(enabled.stacks.api).toJSON()).toEqual(Template.fromStack(baseline.stacks.api).toJSON());
  });

  test.each(["dev", "prod"] as const)("%s denies ENI operations only to each exact function's code, without a role cycle", (stage) => {
    const { compute } = synthesize(stage, configFixture(stage));
    const policies = Object.values(resources(compute, "AWS::IAM::Policy"));
    const allDenies = policies.flatMap((policy) => policy.Properties.PolicyDocument.Statement)
      .filter((statement) => statement.Effect === "Deny" && JSON.stringify(statement.Action).includes("ec2:"));
    expect(allDenies).toHaveLength(4);
    for (const key of DB_KEYS) {
      const name = lambdaFunctionName(key, stage);
      const [functionId, fn] = namedFunction(compute, name);
      const [roleId, attribute] = fn.Properties.Role["Fn::GetAtt"];
      expect(attribute).toBe("Arn");
      const role = resources(compute, "AWS::IAM::Role")[roleId];
      expect(JSON.stringify(role.Properties.ManagedPolicyArns)).toContain("service-role/AWSLambdaVPCAccessExecutionRole");
      const rolePolicies = policies.filter((policy) => policy.Properties.Roles?.some((reference: any) => reference.Ref === roleId));
      const denies = rolePolicies.flatMap((policy) => policy.Properties.PolicyDocument.Statement)
        .filter((statement) => statement.Effect === "Deny");
      expect(denies).toHaveLength(1);
      expect(denies[0]).toEqual({
        Effect: "Deny", Action: expect.arrayContaining(ENI_ACTIONS), Resource: "*",
        Condition: { ArnEquals: { "lambda:SourceFunctionArn": {
          "Fn::Join": ["", ["arn:", { Ref: "AWS::Partition" }, `:lambda:eu-central-1:123456789012:function:${name}`]],
        } } },
      });
      expect([...denies[0].Action].sort()).toEqual([...ENI_ACTIONS].sort());
      expect(JSON.stringify([role, ...rolePolicies])).not.toContain(functionId);
    }
  });

  test.each(["dev", "prod"] as const)("%s delivers exact CA bytes through the actual CLI-credentials staging bucket and stage prefix", (stage) => {
    const config = configFixture(stage);
    const { stacks, assembly, compute } = synthesize(stage, config);
    expect(stacks.compute.synthesizer).toBeInstanceOf(cdk.CliCredentialsStackSynthesizer);
    const [, layer] = only(resources(compute, "AWS::Lambda::LayerVersion"));
    const manifests = fs.readdirSync(assembly.directory).filter((name) => name.endsWith(".assets.json"));
    const assets = manifests.flatMap((name) => {
      const manifest = JSON.parse(fs.readFileSync(path.join(assembly.directory, name), "utf8"));
      return Object.entries(manifest.files ?? {}).filter(([, asset]: [string, any]) => asset.source.packaging === "zip");
    }) as [string, { source: { path: string }; destinations: Record<string, { bucketName: string; objectKey: string; region: string }> }][];
    expect(assets).toHaveLength(1);
    const [hash, asset] = assets[0];
    expect(asset.source.path).toBe(`asset.${hash}`);
    expect(layer.Properties.Content).toEqual({ S3Bucket: CLOUDFORMATION_STAGING_BUCKET_NAME, S3Key: `${stage}/${hash}.zip` });
    expect(Object.values(asset.destinations)).toEqual([expect.objectContaining({
      bucketName: CLOUDFORMATION_STAGING_BUCKET_NAME, objectKey: `${stage}/${hash}.zip`, region: ENVIRONMENT.region,
    })]);
    const staged = path.join(assembly.directory, asset.source.path);
    expect(fs.readdirSync(staged)).toEqual(["postgres-ca"]);
    expect(fs.readdirSync(path.join(staged, "postgres-ca"))).toEqual(["root.pem"]);
    const stagedCa = path.join(staged, "postgres-ca/root.pem");
    expect(fs.readFileSync(stagedCa)).toEqual(Buffer.from(publicCa));
    expect(fs.statSync(stagedCa).mode & 0o7777).toBe(0o644);
    expect(fs.readFileSync(path.join(config.caAssetDirectory, "postgres-ca/root.pem"), "utf8")).toBe(publicCa);
    expect(layer.Properties.CompatibleRuntimes).toEqual(["provided.al2023"]);
    expect(layer.Properties.CompatibleArchitectures).toEqual(["x86_64"]);
    expect(layer.DeletionPolicy).toBe("Retain");
    expect(layer.UpdateReplacePolicy).toBe("Retain");
    expect(compute.Parameters?.BootstrapVersion).toBeUndefined();
    expect(JSON.stringify(compute)).not.toMatch(/cdk-hnb659fds|BEGIN CERTIFICATE|BEGIN PRIVATE KEY/);
    expect(JSON.stringify(compute)).not.toContain(config.caAssetDirectory);
  });

  test.each(["dev", "prod"] as const)("%s caps Shopify polling at its reservation and gates FX schedule on initialization", (stage) => {
    const config = configFixture(stage);
    const { compute } = synthesize(stage, config);
    const [, mapping] = only(resources(compute, "AWS::Lambda::EventSourceMapping"));
    const [shopifyId] = namedFunction(compute, lambdaFunctionName("shopify", stage));
    expect(mapping.Properties.FunctionName).toEqual({ Ref: shopifyId });
    expect(mapping.Properties.ScalingConfig).toEqual({ MaximumConcurrency: config.reservedConcurrency.shopify });
    expect(mapping.Properties.BatchSize).toBe(10);
    expect(mapping.Properties.FunctionResponseTypes).toEqual(["ReportBatchItemFailures"]);
    const [fxId] = namedFunction(compute, lambdaFunctionName("fxRateSync", stage));
    const [initializerId, initializer] = only(Object.fromEntries(Object.entries(compute.Resources)
      .filter(([, resource]) => resource.Properties?.SourceEventId === `deployment:fxrate:initial:${stage}:v1`)));
    expect(initializer.Properties.FunctionName).toEqual({ Ref: fxId });
    const [, schedule] = only(Object.fromEntries(Object.entries(resources(compute, "AWS::Events::Rule"))
      .filter(([, resource]) => resource.Properties.ScheduleExpression !== undefined)));
    expect(schedule.DependsOn).toContain(initializerId);
    expect(schedule.Properties.Targets).toEqual([expect.objectContaining({ Arn: { "Fn::GetAtt": [fxId, "Arn"] } })]);
    const baseline = synthesize(stage).compute;
    const [, oldSchedule] = only(Object.fromEntries(Object.entries(resources(baseline, "AWS::Events::Rule"))
      .filter(([, resource]) => resource.Properties.ScheduleExpression !== undefined)));
    expect(oldSchedule.DependsOn ?? []).not.toContain(initializerId);
    expect(2 * Object.values(config.reservedConcurrency).reduce((sum, value) => sum + value, 0)).toBe(config.lambdaConnectionBudget);
  });

  test.each(STAGES)("%s defaults off without VPC, CA layer, reservations or queue cap", (stage) => {
    const { stacks, templates, compute } = synthesize(stage);
    expect(stacks.data.lambdaEgress).toBeUndefined();
    expectOff(templates, stage);
    expect(Object.values(resources(compute, "AWS::Lambda::Function"))
      .filter((fn) => fn.Properties.Environment?.Variables?.POSTGRES_HOST !== undefined))
      .toHaveLength(stage === "ephemeral" ? 3 : 4);
  });

  test("ephemeral single-stack stays unattached and rejects explicit opt-in", () => {
    const app = appFixture();
    const stack = new ApplicationEphemeralStack(app, "application-ephemeral", { stage: "ephemeral", env: ENVIRONMENT });
    expectOff([Template.fromStack(stack).toJSON() as StackTemplate], "ephemeral");
    expect(app.synth().manifest.missing ?? []).toEqual([]);
    expect(() => new ApplicationEphemeralStack(appFixture(), "Rejected", {
      stage: "ephemeral", env: ENVIRONMENT, postgresLambda: configFixture(),
    })).toThrow("PostgreSQL Lambda egress is not supported in ephemeral stacks.");
  });

  test.each(["prod", "ephemeral"] as const)("rejects dev configuration for application stage %s", (stage) => {
    expect(() => createApplicationStacks(appFixture(), { stage, env: ENVIRONMENT, postgresLambda: configFixture() }))
      .toThrow(/Invalid Postgres Lambda configuration: stage/);
  });

  test.each([
    ["missing environment", undefined],
    ["account mismatch", { ...ENVIRONMENT, account: "210987654321" }],
    ["region mismatch", { ...ENVIRONMENT, region: "eu-west-1" }],
  ] as const)("rejects %s rather than requesting lookups", (_label, env) => {
    expect(() => createApplicationStacks(appFixture(), { stage: "dev", env, postgresLambda: configFixture() }))
      .toThrow("Invalid Lambda egress stack environment.");
  });

  test.each(["config-only", "egress-only", "egress-stage", "egress-account", "egress-region", "compute-account"] as const)
    ("rejects compute attachment mismatch: %s", (mismatch) => {
      const app = appFixture();
      const config = configFixture();
      const data = new ApplicationDataStack(app, "Data", { stage: "dev", env: ENVIRONMENT, postgresLambda: config });
      let lambdaEgress: LambdaEgressOutput | undefined = data.lambdaEgress!;
      if (mismatch === "config-only") lambdaEgress = undefined;
      if (mismatch === "egress-stage") lambdaEgress = { ...lambdaEgress!, stage: "prod" };
      if (mismatch === "egress-account") lambdaEgress = { ...lambdaEgress!, environment: { ...ENVIRONMENT, account: "210987654321" } };
      if (mismatch === "egress-region") lambdaEgress = { ...lambdaEgress!, environment: { ...ENVIRONMENT, region: "eu-west-1" } };
      expect(() => new ApplicationComputeStack(app, "Compute", {
        stage: "dev", env: mismatch === "compute-account" ? { ...ENVIRONMENT, account: "210987654321" } : ENVIRONMENT,
        postgresLambda: mismatch === "egress-only" ? undefined : config,
        lambdaEgress, storage: data.storage, queues: data.queues, search: data.search,
      })).toThrow(mismatch === "config-only" || mismatch === "egress-only"
        ? "PostgreSQL Lambda config and data-stack egress must be supplied together."
        : mismatch === "egress-stage" ? "PostgreSQL Lambda attachment stage mismatch."
          : "PostgreSQL Lambda stack environment mismatch.");
    });
});

// Execute the real entry point, not the CDK CLI: no credential provider, AWS calls or context lookup resolution.
function runEntry(context: Record<string, string>, extraEnvironment: Record<string, string> = {}) {
  const outdir = temporaryDirectory();
  const result = spawnSync(process.execPath, [
    "-r", require.resolve("ts-node/register"), path.resolve(__dirname, "../bin/app.ts"),
  ], {
    cwd: path.resolve(__dirname, ".."),
    env: { ...SAFE_ENV, ...extraEnvironment, CDK_OUTDIR: outdir, CDK_CONTEXT_JSON: JSON.stringify(context) },
    encoding: "utf8", timeout: 20000, maxBuffer: 4 * 1024 * 1024,
  });
  expect(result.error).toBeUndefined();
  expect(result.signal).toBeNull();
  return { result, outdir };
}

function entryTemplates(outdir: string): StackTemplate[] {
  const manifest = JSON.parse(fs.readFileSync(path.join(outdir, "manifest.json"), "utf8"));
  expect(manifest.missing ?? []).toEqual([]);
  const templates = Object.values(manifest.artifacts).filter((artifact: any) => artifact.type === "aws:cloudformation:stack")
    .map((artifact: any) => JSON.parse(fs.readFileSync(path.join(outdir, artifact.properties.templateFile), "utf8")) as StackTemplate);
  expect(templates.length).toBeGreaterThan(0);
  expect(JSON.stringify(templates)).not.toContain("Fn::GetAZs");
  return templates;
}

describe("PostgreSQL Lambda explicit app context", () => {
  test.each(["dev", "prod"] as const)("%s loads explicit JSON and supplies the matching stack environment", (stage) => {
    const config = configFixture(stage);
    const file = path.join(temporaryDirectory(), "postgres-lambda.json");
    fs.writeFileSync(file, JSON.stringify(config));
    const { result, outdir } = runEntry({ stage, postgresLambdaConfig: file });
    expect({ status: result.status, stderr: result.stderr }).toEqual({ status: 0, stderr: expect.any(String) });
    const templates = entryTemplates(outdir);
    expect(templates.flatMap((template) => Object.values(resources(template, "AWS::Lambda::Function")))
      .filter((fn) => fn.Properties.VpcConfig !== undefined)).toHaveLength(4);
    expect(templates.flatMap((template) => Object.values(resources(template, "AWS::Lambda::LayerVersion")))).toHaveLength(1);
    const manifest = JSON.parse(fs.readFileSync(path.join(outdir, "manifest.json"), "utf8"));
    for (const artifact of Object.values(manifest.artifacts) as any[]) {
      if (artifact.type === "aws:cloudformation:stack") expect(artifact.environment).toBe("aws://123456789012/eu-central-1");
    }
  }, 25000);

  test.each(STAGES)("%s entry defaults off and ignores ambient opt-in lookalikes", (stage) => {
    const { result, outdir } = runEntry({ stage }, {
      POSTGRES_LAMBDA_CONFIG: "/not-read/postgres.json", postgresLambdaConfig: "/not-read/postgres.json",
    });
    expect({ status: result.status, stderr: result.stderr }).toEqual({ status: 0, stderr: expect.any(String) });
    expectOff(entryTemplates(outdir), stage);
  }, 25000);

  test.each(STAGES)("%s singleStack rejects opt-in", (stage) => {
    const file = path.join(temporaryDirectory(), "postgres-lambda.json");
    fs.writeFileSync(file, JSON.stringify(configFixture(stage === "prod" ? "prod" : "dev")));
    const { result, outdir } = runEntry({ stage, singleStack: "true", postgresLambdaConfig: file });
    expect(result.status).not.toBe(0);
    expect(result.stderr).toContain(stage === "ephemeral"
      ? "Invalid Postgres Lambda configuration: stage (dev or prod required)"
      : "singleStack mode is only supported for the ephemeral stage.");
    expect(fs.existsSync(path.join(outdir, "manifest.json"))).toBe(false);
  }, 25000);

  test("explicit malformed JSON fails closed instead of reverting to default-off", () => {
    const file = path.join(temporaryDirectory(), "postgres-lambda.json");
    fs.writeFileSync(file, "{invalid");
    const { result } = runEntry({ stage: "dev", postgresLambdaConfig: file });
    expect(result.status).not.toBe(0);
    expect(result.stderr).toContain("Invalid Postgres Lambda configuration: file JSON");
  }, 25000);
});
