import * as fs from "node:fs";
import { isIP } from "node:net";
import { isAbsolute } from "node:path";
import type { StageName } from "./config";
import type { LambdaEgressProps } from "./constructs/lambda-egress";
import { DATABASE_LAMBDA_KEYS, type DatabaseLambdaKey } from "./constructs/lambdas";

export interface PostgresLambdaConfig {
  stage: "dev" | "prod";
  environment: { account: string; region: string };
  network: Omit<LambdaEgressProps, "stage" | "environment">;
  databaseHostname: string;
  caAssetDirectory: string;
  reservedConcurrency: Record<DatabaseLambdaKey, number>;
  lambdaConnectionBudget: number;
}

const MAX_CONFIG_BYTES = 64 * 1024;

function invalid(field: string): never {
  // Only developer-owned field labels reach errors; never values, unknown keys or causes.
  throw new Error(`Invalid Postgres Lambda configuration: ${field}`);
}

function realStage(stage: StageName): asserts stage is "dev" | "prod" {
  if (stage !== "dev" && stage !== "prod") invalid("stage (dev or prod required)");
}

function object(value: unknown, keys: readonly string[], field: string): Record<string, unknown> {
  if (value === null || typeof value !== "object" || Array.isArray(value)) invalid(field);
  const ownKeys = Reflect.ownKeys(value);
  if (ownKeys.length !== keys.length || keys.some((key) => {
    const descriptor = Object.getOwnPropertyDescriptor(value, key);
    return !descriptor || !("value" in descriptor);
  })) invalid(`${field} fields`);
  return value as Record<string, unknown>;
}

function text(value: unknown, field: string): string {
  if (typeof value !== "string" || value.length === 0 || value !== value.trim()
    || /[\x00-\x1f\x7f]/.test(value) || value.includes("${") || value.includes("{{")) invalid(field);
  return value;
}

function absolutePath(value: unknown, field: string): string {
  const path = text(value, field);
  if (!isAbsolute(path)) invalid(field);
  return path;
}

function integer(value: unknown, minimum: number, maximum: number, field: string): number {
  if (typeof value !== "number" || !Number.isSafeInteger(value) || value < minimum || value > maximum) {
    invalid(field);
  }
  return value;
}

function hostname(value: unknown): string {
  const host = text(value, "databaseHostname");
  const labels = host.split(".");
  if (host.length > 253 || labels.some((label) => !/^[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?$/i.test(label))
    || /^\d+$/.test(labels[labels.length - 1])) invalid("databaseHostname (DNS name required)");
  // URL parsing is local only, and catches legacy numeric IP forms such as 0x7f000001.
  try {
    if (isIP(new URL(`https://${host}`).hostname)) invalid("databaseHostname (DNS name required)");
  } catch {
    invalid("databaseHostname (DNS name required)");
  }
  return host;
}

function networkConfig(value: unknown): PostgresLambdaConfig["network"] {
  const network = object(value, [
    "ipProtocol", "vpcCidr", "availabilityZones", "natTopology", "database", "httpsPolicy",
  ], "network");
  if (network.ipProtocol !== "IPV4") invalid("network.ipProtocol");
  const vpcCidr = text(network.vpcCidr, "network.vpcCidr");
  if (!Array.isArray(network.availabilityZones) || network.availabilityZones.length < 1
    || network.availabilityZones.length > 3) invalid("network.availabilityZones");
  const availabilityZones = Array.from(network.availabilityZones, (value: unknown) => {
    const layout = object(value, ["availabilityZone", "publicSubnetCidr", "privateSubnetCidr"], "network.availabilityZones");
    return {
      availabilityZone: text(layout.availabilityZone, "network.availabilityZones.availabilityZone"),
      publicSubnetCidr: text(layout.publicSubnetCidr, "network.availabilityZones.publicSubnetCidr"),
      privateSubnetCidr: text(layout.privateSubnetCidr, "network.availabilityZones.privateSubnetCidr"),
    };
  });

  const natValue = network.natTopology;
  if (natValue === null || typeof natValue !== "object") invalid("network.natTopology");
  const natMode = Object.getOwnPropertyDescriptor(natValue, "mode")?.value;
  let natTopology: PostgresLambdaConfig["network"]["natTopology"];
  if (natMode === "SINGLE") {
    const nat = object(natValue, ["mode", "availabilityZone"], "network.natTopology");
    natTopology = { mode: "SINGLE", availabilityZone: text(nat.availabilityZone, "network.natTopology.availabilityZone") };
  } else if (natMode === "PER_AZ") {
    object(natValue, ["mode"], "network.natTopology");
    natTopology = { mode: "PER_AZ" };
  } else invalid("network.natTopology.mode");

  const db = object(network.database, ["destinationCidr", "port"], "network.database");
  const database = {
    destinationCidr: text(db.destinationCidr, "network.database.destinationCidr"),
    port: integer(db.port, 1, 65535, "network.database.port"),
  };
  if (database.port === 443) invalid("network.database.port");

  const httpsValue = network.httpsPolicy;
  if (httpsValue === null || typeof httpsValue !== "object") invalid("network.httpsPolicy");
  const httpsMode = Object.getOwnPropertyDescriptor(httpsValue, "mode")?.value;
  let httpsPolicy: PostgresLambdaConfig["network"]["httpsPolicy"];
  if (httpsMode === "PUBLIC_IPV4") {
    object(httpsValue, ["mode"], "network.httpsPolicy");
    httpsPolicy = { mode: "PUBLIC_IPV4" };
  } else if (httpsMode === "CIDR_ALLOWLIST") {
    const https = object(httpsValue, ["mode", "destinationCidrs"], "network.httpsPolicy");
    if (!Array.isArray(https.destinationCidrs) || https.destinationCidrs.length < 1
      || https.destinationCidrs.length > 50) invalid("network.httpsPolicy.destinationCidrs");
    httpsPolicy = {
      mode: "CIDR_ALLOWLIST",
      destinationCidrs: Array.from(https.destinationCidrs, (cidr: unknown) => text(cidr, "network.httpsPolicy.destinationCidrs")),
    };
  } else invalid("network.httpsPolicy.mode");

  // LambdaEgress remains responsible for CIDR semantics, overlaps, AZs and stack matching.
  return { ipProtocol: "IPV4", vpcCidr, availabilityZones, natTopology, database, httpsPolicy };
}

export function parsePostgresLambdaConfig(value: unknown, stage: StageName): PostgresLambdaConfig {
  realStage(stage);
  const config = object(value, [
    "stage", "environment", "network", "databaseHostname", "caAssetDirectory", "reservedConcurrency", "lambdaConnectionBudget",
  ], "configuration");
  if (config.stage !== stage) invalid("stage (must match application stage)");
  const environment = object(config.environment, ["account", "region"], "environment");
  const account = text(environment.account, "environment.account");
  if (!/^\d{12}$/.test(account) || account === "000000000000") invalid("environment.account");
  const region = text(environment.region, "environment.region");
  if (!/^[a-z]{2}(?:-[a-z]+)+-[1-9]\d*$/.test(region)) invalid("environment.region");
  const network = networkConfig(config.network);
  const databaseHostname = hostname(config.databaseHostname);
  const caAssetDirectory = absolutePath(config.caAssetDirectory, "caAssetDirectory (absolute directory required)");
  const reservations = object(config.reservedConcurrency, DATABASE_LAMBDA_KEYS, "reservedConcurrency");
  const reservedConcurrency = {} as Record<DatabaseLambdaKey, number>;
  let sum = 0;
  for (const key of DATABASE_LAMBDA_KEYS) {
    // Conservative per-function ceiling, not a claim about available account quota.
    const count = integer(reservations[key], key === "shopify" ? 2 : 1, 1000, "reservedConcurrency (1..1000; Shopify 2..1000)");
    reservedConcurrency[key] = count;
    sum += count;
  }
  const minimumBudget = 2 * sum;
  if (!Number.isSafeInteger(minimumBudget)) invalid("lambdaConnectionBudget");
  const lambdaConnectionBudget = integer(config.lambdaConnectionBudget, minimumBudget, Number.MAX_SAFE_INTEGER,
    "lambdaConnectionBudget (safe integer, at least twice total reserved concurrency)");
  return { stage, environment: { account, region }, network, databaseHostname, caAssetDirectory, reservedConcurrency, lambdaConnectionBudget };
}

/** Explicit local JSON only: no environment fallback, lookups or asset validation. */
export function loadPostgresLambdaConfig(file: unknown, stage: StageName): PostgresLambdaConfig {
  realStage(stage);
  const path = absolutePath(file, "file (absolute path required)");
  let contents: string;
  try {
    // Nonblocking open prevents a FIFO input from hanging before the regular-file check.
    const fd = fs.openSync(path, fs.constants.O_RDONLY | fs.constants.O_NONBLOCK);
    try {
      const stat = fs.fstatSync(fd);
      if (!stat.isFile() || stat.size > MAX_CONFIG_BYTES) invalid("file size or type");
      // Bound the read even if the file grows after fstat; one extra byte detects overflow.
      const buffer = Buffer.alloc(MAX_CONFIG_BYTES + 1);
      let length = 0;
      while (length < buffer.length) {
        const read = fs.readSync(fd, buffer, length, buffer.length - length, null);
        if (read === 0) break;
        length += read;
      }
      if (length > MAX_CONFIG_BYTES) invalid("file size");
      contents = new TextDecoder("utf-8", { fatal: true }).decode(buffer.subarray(0, length));
    } finally {
      fs.closeSync(fd);
    }
  } catch {
    invalid("file (readable UTF-8 regular file, at most 65536 bytes required)");
  }
  let value: unknown;
  try {
    value = JSON.parse(contents);
  } catch {
    invalid("file JSON");
  }
  return parsePostgresLambdaConfig(value, stage);
}
