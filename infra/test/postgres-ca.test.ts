import * as cdk from "aws-cdk-lib";
import * as lambda from "aws-cdk-lib/aws-lambda";
import { Template } from "aws-cdk-lib/assertions";
import { execFileSync } from "node:child_process";
import { X509Certificate } from "node:crypto";
import * as fs from "node:fs";
import * as os from "node:os";
import * as path from "node:path";
import { inspect } from "node:util";
import { PostgresCa, POSTGRES_CA_PATH } from "../src/constructs/postgres-ca";

const SAFE_ENV = {
  PATH: process.env.PATH,
  AWS_CONFIG_FILE: "/dev/null",
  AWS_SHARED_CREDENTIALS_FILE: "/dev/null",
  AWS_EC2_METADATA_DISABLED: "true",
};
const ENVIRONMENT = { account: "123456789012", region: "eu-central-1" };
const temporaryDirectories: string[] = [];
let oldCa: string;
let newCa: string;
let leaf: string;
let originalUmask: number;

function temporaryDirectory(): string {
  const directory = fs.mkdtempSync(path.join(os.tmpdir(), "postgres-ca-sensitive-path-"));
  fs.chmodSync(directory, 0o755);
  temporaryDirectories.push(directory);
  return directory;
}

function publicCertificate(name: string, ca: boolean, days = 3650): string {
  // OpenSSL signs locally and discards its disposable private key into /dev/null.
  return execFileSync("openssl", [
    "req", "-new", "-x509", "-newkey", "ec", "-pkeyopt", "ec_paramgen_curve:prime256v1",
    "-nodes", "-keyout", "/dev/null", "-subj", `/CN=Synthetic test ${name}`,
    "-days", String(days), "-addext", `basicConstraints=critical,CA:${ca ? "TRUE" : "FALSE"}`,
    "-addext", `keyUsage=critical,${ca ? "keyCertSign,cRLSign" : "digitalSignature"}`,
  ], {
    env: SAFE_ENV,

    encoding: "utf8",
    timeout: 10000,
    stdio: ["pipe", "pipe", "pipe"],
  });
}

function source(pem: string | Buffer = oldCa) {
  const directory = temporaryDirectory();
  const subdirectory = path.join(directory, "postgres-ca");
  const file = path.join(subdirectory, "root.pem");
  fs.mkdirSync(subdirectory);
  fs.chmodSync(subdirectory, 0o755);
  fs.writeFileSync(file, pem);
  fs.chmodSync(file, 0o644);
  return { directory, subdirectory, file };
}

function stackFixture() {
  const app = new cdk.App({ outdir: temporaryDirectory(), analyticsReporting: false });
  const stack = new cdk.Stack(app, "CaTest", { env: ENVIRONMENT });
  return { app, stack };
}

function synthesize(directory: string) {
  const { app, stack } = stackFixture();
  const construct = new PostgresCa(stack, "Ca", { assetDirectory: directory });
  expect(construct.layer).toBeInstanceOf(lambda.LayerVersion);
  const assembly = app.synth();
  const template = Template.fromStack(stack);
  const resources = template.findResources("AWS::Lambda::LayerVersion");
  expect(Object.keys(resources)).toHaveLength(1);
  const [logicalId, resource] = Object.entries(resources)[0];
  const manifests = fs.readdirSync(assembly.directory).filter((name) => name.endsWith(".assets.json"));
  expect(manifests).toHaveLength(1);
  const manifest = JSON.parse(fs.readFileSync(path.join(assembly.directory, manifests[0]), "utf8")) as {
    files: Record<string, {
      source: { path: string; packaging: string };
      destinations: Record<string, { objectKey: string }>;
    }>;
  };
  const assets = Object.entries(manifest.files).filter(([, asset]) => asset.source.packaging === "zip");
  expect(assets).toHaveLength(1);
  const [hash, asset] = assets[0];
  expect(asset.source.path).toBe(`asset.${hash}`);
  expect(resource.Properties.Content.S3Key).toBe(`${hash}.zip`);
  expect(Object.values(asset.destinations).map((destination) => destination.objectKey)).toEqual([`${hash}.zip`]);
  expect(assembly.manifest.missing ?? []).toEqual([]);
  return { template, logicalId, resource, hash, staged: path.join(assembly.directory, asset.source.path) };
}

function expectRejected(directory: string, category: string, forbidden: string[] = []) {
  const { stack } = stackFixture();
  let failure: unknown;
  try {
    new PostgresCa(stack, "Ca", { assetDirectory: directory });
  } catch (error) {
    failure = error;
  }
  expect(failure).toBeInstanceOf(Error);
  const error = failure as Error;
  expect(error.message).toBe(`PostgresCa: ${category}`);
  expect(error.cause).toBeUndefined();
  for (const value of [directory, ...forbidden].filter((value) => value.length > 0)) {
    expect(inspect(error)).not.toContain(value);
    expect(JSON.stringify(error)).not.toContain(value);
  }
}

function restoreDirectories(directory: string): void {
  if (!fs.lstatSync(directory).isDirectory()) return;
  fs.chmodSync(directory, 0o755);
  for (const name of fs.readdirSync(directory)) restoreDirectories(path.join(directory, name));
}

beforeAll(() => {
  originalUmask = process.umask(0o022);
  oldCa = publicCertificate("old CA", true);
  newCa = publicCertificate("new CA", true, 7300);
  leaf = publicCertificate("leaf", false);
  for (const pem of [oldCa, newCa]) {
    const certificate = new X509Certificate(pem);
    expect(certificate.ca).toBe(true);
    expect(certificate.validFromDate.getTime()).toBeLessThanOrEqual(Date.now());
    expect(certificate.validToDate.getTime()).toBeGreaterThan(Date.now());
  }
});

afterAll(() => process.umask(originalUmask));

afterEach(() => {
  jest.restoreAllMocks();
  for (const directory of temporaryDirectories.splice(0)) {
    restoreDirectories(directory);
    fs.rmSync(directory, { recursive: true, force: true });
  }
});

describe("PostgresCa offline layer delivery", () => {
  test.each([0o644, 0o444])("packages exact public CA bytes and Lambda-readable modes (file %i)", (mode) => {
    const input = source();
    fs.chmodSync(input.file, mode);
    const { template, resource, staged } = synthesize(input.directory);
    expect(POSTGRES_CA_PATH).toBe("/opt/postgres-ca/root.pem");
    expect(fs.readdirSync(staged)).toEqual(["postgres-ca"]);
    expect(fs.readdirSync(path.join(staged, "postgres-ca"))).toEqual(["root.pem"]);
    const packaged = path.join(staged, path.posix.relative("/opt", POSTGRES_CA_PATH));
    expect(fs.readFileSync(packaged)).toEqual(Buffer.from(oldCa));
    expect(fs.lstatSync(packaged).isFile()).toBe(true);
    expect(fs.statSync(packaged).mode & 0o7777).toBe(mode);
    expect(fs.statSync(path.dirname(packaged)).mode & 0o7777).toBe(0o755);
    expect(fs.statSync(staged).mode & 0o7777).toBe(0o755);
    expect(resource.Properties.CompatibleRuntimes).toEqual(["provided.al2023"]);
    expect(resource.Properties.CompatibleArchitectures).toEqual(["x86_64"]);
    expect(resource.DeletionPolicy).toBe("Retain");
    expect(resource.UpdateReplacePolicy).toBe("Retain");
    // Deployment uses the normal CDK asset bucket, not a runtime S3/SSM fetch or IAM grant.
    expect(Object.values(template.toJSON().Resources).map((entry: any) => entry.Type)).toEqual(["AWS::Lambda::LayerVersion"]);
    expect(JSON.stringify(template.toJSON().Resources)).not.toMatch(/ssm|secretsmanager|AWS::IAM|Custom::|Environment|BEGIN CERTIFICATE/i);
    // CDK's deploy-time bootstrap version check is not CA retrieval or runtime access.
    expect(Object.keys(template.toJSON().Parameters ?? {})).toEqual(["BootstrapVersion"]);
    expect(template.toJSON().Outputs).toBeUndefined();
    expect(fs.readFileSync(input.file, "utf8")).toBe(oldCa);
    expect(fs.statSync(input.file).mode & 0o7777).toBe(mode);
  });

  test("accepts old+new rotation bundle, ASCII spacing and read-only directories", () => {
    const pem = ` \t\r\n${oldCa.replace(/\n/g, "\r\n")}\n${newCa}\t\n`;
    const input = source(pem);
    fs.chmodSync(input.subdirectory, 0o555);
    fs.chmodSync(input.directory, 0o555);
    const { staged } = synthesize(input.directory);
    expect(fs.readFileSync(path.join(staged, "postgres-ca/root.pem"), "utf8")).toBe(pem);
  });

  test("changed CA at the same source path replaces content hash, retaining previous staged bytes", () => {
    const input = source();
    const original = synthesize(input.directory);
    fs.writeFileSync(input.file, newCa);
    const rotated = synthesize(input.directory);
    expect(rotated.hash).not.toBe(original.hash);
    expect(rotated.resource.Properties.Content.S3Key).not.toBe(original.resource.Properties.Content.S3Key);
    expect(rotated.logicalId).toBe(original.logicalId);
    expect(rotated.resource.UpdateReplacePolicy).toBe("Retain");
    expect(fs.readFileSync(path.join(original.staged, "postgres-ca/root.pem"), "utf8")).toBe(oldCa);
    expect(fs.readFileSync(path.join(rotated.staged, "postgres-ca/root.pem"), "utf8")).toBe(newCa);
    expect(synthesize(source(newCa).directory).hash).toBe(rotated.hash);
  });

  test("accepts a literal relative asset directory", () => {
    const input = source();
    synthesize(path.relative(process.cwd(), input.directory));
  });

  test.each(["", " \t", "bad\0directory"])("rejects invalid literal directory: %j", (directory) => {
    expectRejected(directory, "INVALID_DIRECTORY");
  });

  test("rejects CDK tokens, including embedded tokens", () => {
    const { stack } = stackFixture();
    const token = new cdk.CfnParameter(stack, "OperatorInput").valueAsString;
    expectRejected(token, "INVALID_DIRECTORY");
    expectRejected(`/sensitive/${token}/ca`, "INVALID_DIRECTORY");
  });

  test("rejects missing directory without filesystem paths or causes", () => {
    expectRejected(path.join(temporaryDirectory(), "operator-private-path"), "SOURCE_IO");
  });

  test.each(["postgres-ca", "postgres-ca/root.pem"])("rejects missing required entry %s", (entry) => {
    const input = source();
    fs.rmSync(path.join(input.directory, entry), { recursive: true });
    expectRejected(input.directory, "INVALID_TREE");
  });

  test.each(["", "postgres-ca", "postgres-ca/root.pem"])("rejects wrong file type at %s", (entry) => {
    const input = source();
    const target = path.join(input.directory, entry);
    fs.rmSync(target, { recursive: true });
    if (entry === "postgres-ca/root.pem") fs.mkdirSync(target);
    else fs.writeFileSync(target, oldCa);
    expectRejected(input.directory, "INVALID_TREE");
  });

  test.each(["secret.key", ".is_custom_resource", "postgres-ca/extra.pem", "postgres-ca/extra-directory"])("rejects extraneous entry %s", (entry) => {
    const input = source();
    if (entry.endsWith("directory")) fs.mkdirSync(path.join(input.directory, entry));
    else fs.writeFileSync(path.join(input.directory, entry), "sensitive-provider-body");
    expectRejected(input.directory, "INVALID_TREE", ["sensitive-provider-body"]);
  });

  test.each(["", "postgres-ca", "postgres-ca/root.pem"])("rejects symlink at %s", (entry) => {
    const input = source();
    const target = source();
    const link = path.join(input.directory, entry);
    fs.rmSync(link, { recursive: true });
    fs.symlinkSync(path.join(target.directory, entry), link);
    expectRejected(input.directory, "INVALID_TREE");
  });

  test("rejects symlinked ancestor and dangling file symlink", () => {
    const input = source();
    const parent = temporaryDirectory();
    fs.symlinkSync(path.dirname(input.directory), path.join(parent, "linked"));
    expectRejected(path.join(parent, "linked", path.basename(input.directory)), "INVALID_TREE");
    fs.rmSync(input.file);
    fs.symlinkSync(path.join(parent, "missing"), input.file);
    expectRejected(input.directory, "INVALID_TREE");
  });

  test.each([0o000, 0o400, 0o600, 0o640, 0o660, 0o666, 0o755, 0o4644])("rejects unsafe/unreadable file mode %i", (mode) => {
    const input = source();
    fs.chmodSync(input.file, mode);
    expectRejected(input.directory, "UNSAFE_PERMISSIONS");
  });

  test.each([0o000, 0o700, 0o750, 0o744, 0o775, 0o777, 0o1755])("rejects unsafe/untraversable directory mode %i", (mode) => {
    for (const entry of ["", "postgres-ca"]) {
      const input = source();
      fs.chmodSync(path.join(input.directory, entry), mode);
      expectRejected(input.directory, "UNSAFE_PERMISSIONS");
    }
  });

  test.each([0o077, 0o000, 0o002])("rejects unsafe staging umask %i even when source modes are safe", (mask) => {
    const input = source();
    const previous = process.umask(mask);
    try {
      expectRejected(input.directory, "UNSAFE_PERMISSIONS");
    } finally {
      process.umask(previous);
    }
  });

  test.each([0, 1024 * 1024 + 1])("rejects invalid byte size %i", (size) => {
    expectRejected(source(Buffer.alloc(size, " ")).directory, "INVALID_SIZE");
  });

  test("accepts the 1 MiB limit, rejects an otherwise valid bundle one byte over", () => {
    const pem = oldCa.padEnd(1024 * 1024, " ");
    const { staged } = synthesize(source(pem).directory);
    expect(fs.statSync(path.join(staged, "postgres-ca/root.pem")).size).toBe(1024 * 1024);
    expectRejected(source(`${pem} `).directory, "INVALID_SIZE");
  });

  test.each([
    " \t\r\n", "sensitive-provider-body", "-----BEGIN CERTIFICATE-----\n???\n-----END CERTIFICATE-----",
    "-----BEGIN CERTIFICATE-----\nYWJj\n-----END CERTIFICATE-----", "-----BEGIN CERTIFICATE-----\nYWJj",
    "-----BEGIN PRIVATE KEY-----\nsensitive-key-marker\n-----END PRIVATE KEY-----",
    "-----BEGIN RSA PRIVATE KEY-----\nsensitive-key-marker\n-----END RSA PRIVATE KEY-----",
    "-----BEGIN EC PRIVATE KEY-----\nsensitive-key-marker\n-----END EC PRIVATE KEY-----",
    "-----BEGIN ENCRYPTED PRIVATE KEY-----\nsensitive-key-marker\n-----END ENCRYPTED PRIVATE KEY-----",
  ])("rejects non-certificate content case %# without exposing it", (pem) => {
    for (const bundle of [pem, oldCa + pem, pem + oldCa]) {
      // Whitespace around a real certificate is legal; whitespace alone is not.
      if (pem.trim() === "" && bundle !== pem) continue;
      expectRejected(source(bundle).directory, "INVALID_PEM", ["sensitive-provider-body", "sensitive-key-marker"]);
    }
  });

  test("rejects invalid UTF-8, noncanonical base64 and trailing DER bytes", () => {
    const raw = new X509Certificate(oldCa).raw;
    for (const pem of [
      Buffer.concat([Buffer.from(oldCa), Buffer.from([0xff])]),
      oldCa.replace("-----END", "=\n-----END"),
      `-----BEGIN CERTIFICATE-----\n${Buffer.concat([raw, Buffer.from("provider-body")]).toString("base64")}\n-----END CERTIFICATE-----\n`,
      oldCa.replace("CERTIFICATE", "TRUSTED CERTIFICATE"),
      oldCa.replace("-----\n", "----- "),
      oldCa.replace("\n-----END", "-----END"),
    ]) expectRejected(source(pem).directory, "INVALID_PEM", ["provider-body"]);
  });

  test("rejects leaf certificates, including either position in a bundle", () => {
    for (const pem of [leaf, oldCa + leaf, leaf + oldCa]) {
      expectRejected(source(pem).directory, "NOT_CA", [leaf]);
    }
  });

  test.each(["expired", "not-yet-valid"])("rejects %s certificates using a controlled validation clock", (state) => {
    const certificate = new X509Certificate(oldCa);
    const now = state === "expired" ? certificate.validToDate.getTime() : certificate.validFromDate.getTime() - 1;
    jest.spyOn(Date, "now").mockReturnValue(now);
    expectRejected(source(oldCa).directory, "INVALID_VALIDITY", [oldCa]);
    if (state === "expired") {
      expect(new X509Certificate(newCa).validToDate.getTime()).toBeGreaterThan(now);
      synthesize(source(newCa).directory);
    }
    expectRejected(source(newCa + oldCa).directory, "INVALID_VALIDITY", [oldCa]);
  });

  test("sanitizes CDK asset staging failures without retaining raw causes", () => {
    jest.spyOn(lambda.Code, "fromAsset").mockImplementation(() => {
      throw new Error("sensitive-provider-body /operator/private-path");
    });
    expectRejected(source().directory, "ASSET_STAGING", ["sensitive-provider-body", "/operator/private-path"]);
  });
});