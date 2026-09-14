import fs = require("node:fs");
import { inspect } from "node:util";
import type { StageName } from "../src/config";
import { DATABASE_LAMBDA_KEYS, type DatabaseLambdaKey } from "../src/constructs/lambdas";
import {
  loadPostgresLambdaConfig,
  parsePostgresLambdaConfig,
  type PostgresLambdaConfig,
} from "../src/postgres-lambda-config";

const SECRET = "redaction-sentinel-password";
const FILE = `/operator/${SECRET}/postgres.json`;
const MAX_BYTES = 64 * 1024;

// Syntax fixtures only, not deployment inputs. No network or real file access.
function fixture(stage: "dev" | "prod" = "dev"): PostgresLambdaConfig {
  const reservedConcurrency = Object.fromEntries(DATABASE_LAMBDA_KEYS.map((key) => [key, 2])) as Record<DatabaseLambdaKey, number>;
  return {
    stage,
    environment: { account: "123456789012", region: "eu-central-1" },
    network: {
      ipProtocol: "IPV4",
      vpcCidr: "10.42.0.0/16",
      availabilityZones: [
        { availabilityZone: "eu-central-1a", publicSubnetCidr: "10.42.0.0/24", privateSubnetCidr: "10.42.16.0/24" },
      ],
      natTopology: { mode: "SINGLE", availabilityZone: "eu-central-1a" },
      database: { destinationCidr: "8.8.8.8/32", port: 5432 },
      httpsPolicy: { mode: "PUBLIC_IPV4" },
    },
    databaseHostname: "postgres.example.test",
    caAssetDirectory: "/operator/ca",
    reservedConcurrency,
    lambdaConnectionBudget: 4 * DATABASE_LAMBDA_KEYS.length,
  };
}

function expectInvalid(action: () => unknown, field?: string): void {
  let failure: unknown;
  try {
    action();
  } catch (error) {
    failure = error;
  }
  expect(failure).toBeInstanceOf(Error);
  const error = failure as Error;
  expect(error.message).toMatch(/^Invalid Postgres Lambda configuration: /);
  if (field) expect(error.message).toContain(field);
  expect(error.cause).toBeUndefined();
  expect(inspect(error, { depth: null })).not.toContain(SECRET);
}

describe("parsePostgresLambdaConfig", () => {
  test.each(["dev", "prod"] as const)("accepts explicit %s config without defaults or mutation", (stage) => {
    const input = fixture(stage);
    const before = structuredClone(input);
    const parsed = parsePostgresLambdaConfig(input, stage);
    expect(parsed).toEqual(before);
    expect(input).toEqual(before);
    expect(parsed).not.toBe(input);
    expect(parsed.network).not.toBe(input.network);
    expect(parsed.reservedConcurrency).not.toBe(input.reservedConcurrency);
    expect(Object.keys(parsed.reservedConcurrency).sort()).toEqual([...DATABASE_LAMBDA_KEYS].sort());
  });

  test("accepts PER_AZ and explicit HTTPS allowlist", () => {
    const input = fixture("prod");
    input.network = { ...input.network, natTopology: { mode: "PER_AZ" },
      httpsPolicy: { mode: "CIDR_ALLOWLIST", destinationCidrs: ["1.1.1.1/32"] } };
    expect(parsePostgresLambdaConfig(input, "prod")).toEqual(input);
  });

  test.each([undefined, null, true, 42, SECRET, [], new Date()])("rejects malformed root %#", (value) => {
    expectInvalid(() => parsePostgresLambdaConfig(value, "dev"));
  });

  // Exercise exact keys on every object, including both discriminated network variants.
  const objects: [string, (input: any) => any][] = [
    ["root", (input) => input],
    ["environment", (input) => input.environment],
    ["network", (input) => input.network],
    ["AZ layout", (input) => input.network.availabilityZones[0]],
    ["SINGLE NAT", (input) => input.network.natTopology],
    ["database", (input) => input.network.database],
    ["PUBLIC_IPV4 HTTPS", (input) => input.network.httpsPolicy],
    ["PER_AZ NAT", (input) => (input.network.natTopology = { mode: "PER_AZ" })],
    ["CIDR_ALLOWLIST HTTPS", (input) => (input.network.httpsPolicy = { mode: "CIDR_ALLOWLIST", destinationCidrs: ["1.1.1.1/32"] })],
    ["reservations", (input) => input.reservedConcurrency],
  ];

  test.each(objects)("rejects every missing field in %s", (_label, select) => {
    for (const key of Object.keys(select(fixture()))) {
      const input = fixture();
      delete select(input)[key];
      expectInvalid(() => parsePostgresLambdaConfig(input, "dev"));
    }
  });

  test.each(objects)("rejects and redacts extra/unknown fields in %s", (_label, select) => {
    const input = fixture();
    select(input)[SECRET] = SECRET;
    expectInvalid(() => parsePostgresLambdaConfig(input, "dev"));
  });

  test("rejects unknown key replacing a required field", () => {
    const input: any = fixture();
    delete input.environment.account;
    input.environment[SECRET] = SECRET;
    expectInvalid(() => parsePostgresLambdaConfig(input, "dev"), "environment");
  });

  test("rejects inherited required fields and accessors without invoking them", () => {
    const input = fixture();
    const inherited = Object.assign(Object.create({ account: input.environment.account }), { region: input.environment.region });
    expectInvalid(() => parsePostgresLambdaConfig({ ...input, environment: inherited }, "dev"));
    Object.defineProperty(input, "databaseHostname", { get: () => { throw new Error(SECRET); } });
    expectInvalid(() => parsePostgresLambdaConfig(input, "dev"));
  });

  test.each([undefined, null, "ephemeral", "local", "test", "DEV", SECRET, "prod"])("rejects config stage %#", (stage) => {
    expectInvalid(() => parsePostgresLambdaConfig({ ...fixture(), stage }, "dev"), "stage");
  });

  test.each([undefined, null, "ephemeral", "local", SECRET])("rejects non-real application stage %#", (stage) => {
    expectInvalid(() => parsePostgresLambdaConfig(fixture(), stage as StageName), "stage");
  });

  test("rejects dev config for prod application", () => {
    expectInvalid(() => parsePostgresLambdaConfig(fixture(), "prod"), "stage");
  });

  test.each([undefined, null, 123456789012, "000000000000", "12345678901", "1234567890123", "12345678901x", " 123456789012", SECRET])(
    "rejects account %#", (account) => {
      const input = fixture();
      expectInvalid(() => parsePostgresLambdaConfig({ ...input, environment: { ...input.environment, account } }, "dev"), "environment.account");
    },
  );

  test.each([undefined, null, 1, "", "eu-central", "EU-CENTRAL-1", "eu-central-0", "eu-central-1a", "eu-central-1\n", "${Token[secret]}", SECRET])(
    "rejects region %#", (region) => {
      const input = fixture();
      expectInvalid(() => parsePostgresLambdaConfig({ ...input, environment: { ...input.environment, region } }, "dev"), "environment.region");
    },
  );

  test.each(["us-east-1", "us-gov-west-1", "cn-north-1"])("accepts region syntax %s without lookup", (region) => {
    const input = fixture();
    input.environment.region = region;
    // AZ/region consistency is deliberately checked later by LambdaEgress.
    expect(parsePostgresLambdaConfig(input, "dev").environment.region).toBe(region);
  });

  test.each([
    undefined, null, 123, "", `postgres://${SECRET}@postgres.example.test/db`, "https://postgres.example.test",
    `${SECRET}@postgres.example.test`, "postgres.example.test:5432", "postgres.example.test/path",
    "postgres.example.test?sslmode=disable", "postgres.example.test#fragment", "127.0.0.1", "127.1",
    "2130706433", "0x7f000001", "0x7f.0x1", "::1", "[::1]", "db..example.test", "-db.example.test",
    "db-.example.test", "db_name.example.test", "*.example.test", "db.example.test.", " db.example.test",
    "db.example.test\n", "${Token[secret]}", `${"a".repeat(64)}.example.test`, `${"a".repeat(63)}.`.repeat(4) + "test",
  ])("rejects hostname %# without leaking input", (databaseHostname) => {
    expectInvalid(() => parsePostgresLambdaConfig({ ...fixture(), databaseHostname }, "dev"), "databaseHostname");
  });

  test.each(["postgres.example.test", "Postgres-1.Example.test", "postgres", `${"a".repeat(63)}.example.test`])(
    "accepts DNS hostname %s", (databaseHostname) => {
      expect(parsePostgresLambdaConfig({ ...fixture(), databaseHostname }, "dev").databaseHostname).toBe(databaseHostname);
    },
  );

  test.each([undefined, null, 123, "", "relative/ca", "~/ca", "file:///operator/ca", " /operator/ca", "/operator/ca\n", `/operator/${SECRET}\0`, "/${Token[secret]}/ca"])(
    "rejects CA path %#", (caAssetDirectory) => {
      expectInvalid(() => parsePostgresLambdaConfig({ ...fixture(), caAssetDirectory }, "dev"), "caAssetDirectory");
    },
  );

  test("does not read or validate the CA asset directory", () => {
    const caAssetDirectory = `/nonexistent/${SECRET}/ca`;
    expect(parsePostgresLambdaConfig({ ...fixture(), caAssetDirectory }, "dev").caAssetDirectory).toBe(caAssetDirectory);
  });

  const malformedNetwork: [string, unknown][] = [
    ["network", null], ["network", []], ["network.ipProtocol", "DUAL_STACK"],
    ["network.vpcCidr", 123], ["network.vpcCidr", ""], ["network.vpcCidr", "${Token[secret]}"],
    ["network.availabilityZones", null], ["network.availabilityZones", []],
    ["network.availabilityZones", [null]], ["network.availabilityZones", new Array(1)],
    ["network.availabilityZones", new Array(4).fill({})],
    ["network.availabilityZones.0.availabilityZone", 1], ["network.availabilityZones.0.publicSubnetCidr", null],
    ["network.availabilityZones.0.privateSubnetCidr", ""],
    ["network.natTopology", null], ["network.natTopology", []], ["network.natTopology.mode", SECRET],
    ["network.natTopology.availabilityZone", false], ["network.database", null],
    ["network.database.destinationCidr", []], ["network.database.port", "5432"], ["network.database.port", 443],
    ["network.database.port", 0], ["network.database.port", 65536], ["network.database.port", 1.5],
    ["network.httpsPolicy", null], ["network.httpsPolicy.mode", SECRET],
    ["network.httpsPolicy", { mode: "PUBLIC_IPV4", destinationCidrs: [] }],
    ["network.natTopology", { mode: "PER_AZ", availabilityZone: "eu-central-1a" }],
    ...[null, "1.1.1.1/32", [], [null], new Array(1), new Array(51).fill("1.1.1.1/32")].map(
      (destinationCidrs): [string, unknown] => ["network.httpsPolicy", { mode: "CIDR_ALLOWLIST", destinationCidrs }],
    ),
  ];
  test.each(malformedNetwork)("rejects malformed %s %#", (path, value) => {
    const input: any = fixture();
    const parts = path.split(".");
    const parent = parts.slice(0, -1).reduce((object, key) => object[key], input);
    parent[parts[parts.length - 1]] = value;
    expectInvalid(() => parsePostgresLambdaConfig(input, "dev"), "network");
  });

  test.each([undefined, null, [], {}, { cloudWatchLogRetention: 2 }])("rejects malformed/inexact reservations %#", (reservedConcurrency) => {
    expectInvalid(() => parsePostgresLambdaConfig({ ...fixture(), reservedConcurrency }, "dev"), "reservedConcurrency");
  });

  test.each(DATABASE_LAMBDA_KEYS)("enforces positive bounded integer reservation for %s", (key) => {
    for (const value of [undefined, null, "2", true, 0, -1, 1.5, 1001, NaN, Infinity, Number.MAX_SAFE_INTEGER + 1]) {
      const input = fixture();
      expectInvalid(() => parsePostgresLambdaConfig({ ...input, reservedConcurrency: { ...input.reservedConcurrency, [key]: value } }, "dev"), "reservedConcurrency");
    }
    const minimum = key === "shopify" ? 2 : 1;
    for (const value of [minimum, 1000]) {
      const input = fixture();
      input.reservedConcurrency[key] = value;
      input.lambdaConnectionBudget = 2 * Object.values(input.reservedConcurrency).reduce((sum, count) => sum + count, 0);
      expect(parsePostgresLambdaConfig(input, "dev")).toEqual(input);
    }
  });

  test("Shopify requires at least two for SQS maximum concurrency", () => {
    const input = fixture();
    input.reservedConcurrency.shopify = 1;
    expectInvalid(() => parsePostgresLambdaConfig(input, "dev"), "reservedConcurrency");
  });

  test.each([undefined, null, "100", true, 0, -1, 1.5, NaN, Infinity, Number.MAX_SAFE_INTEGER + 1])(
    "rejects invalid budget %#", (lambdaConnectionBudget) => {
      expectInvalid(() => parsePostgresLambdaConfig({ ...fixture(), lambdaConnectionBudget }, "dev"), "lambdaConnectionBudget");
    },
  );

  test("budget must cover twice all reservations; accepts safe-integer ceiling", () => {
    const input = fixture();
    expectInvalid(() => parsePostgresLambdaConfig({ ...input, lambdaConnectionBudget: input.lambdaConnectionBudget - 1 }, "dev"), "lambdaConnectionBudget");
    for (const lambdaConnectionBudget of [input.lambdaConnectionBudget, input.lambdaConnectionBudget + 1, Number.MAX_SAFE_INTEGER]) {
      expect(parsePostgresLambdaConfig({ ...input, lambdaConnectionBudget }, "dev").lambdaConnectionBudget).toBe(lambdaConnectionBudget);
    }
  });
});

describe("loadPostgresLambdaConfig (mocked local filesystem)", () => {
  let data: Buffer;
  let cursor: number;
  let regular: boolean;
  let reportedSize: number | undefined;

  beforeEach(() => {
    data = Buffer.from(JSON.stringify(fixture()));
    cursor = 0;
    regular = true;
    reportedSize = undefined;
    jest.spyOn(fs, "openSync").mockReturnValue(123);
    jest.spyOn(fs, "fstatSync").mockImplementation(() => ({
      isFile: () => regular, size: reportedSize ?? data.length,
    }) as fs.Stats);
    jest.spyOn(fs, "readSync").mockImplementation(((_fd: number, buffer: Buffer, offset: number, length: number) => {
      // Partial reads exercise the loop rather than assuming a single read fills the buffer.
      const count = data.copy(buffer, offset, cursor, Math.min(cursor + length, cursor + 97, data.length));
      cursor += count;
      return count;
    }) as typeof fs.readSync);
    jest.spyOn(fs, "closeSync").mockImplementation(() => undefined);
  });

  afterEach(() => jest.restoreAllMocks());

  test.each(["dev", "prod"] as const)("loads explicit %s JSON and closes file", (stage) => {
    data = Buffer.from(JSON.stringify(fixture(stage)));
    expect(loadPostgresLambdaConfig(FILE, stage)).toEqual(fixture(stage));
    expect(fs.openSync).toHaveBeenCalledWith(FILE, fs.constants.O_RDONLY | fs.constants.O_NONBLOCK);
    expect(fs.readSync).toHaveBeenCalledTimes(Math.ceil(data.length / 97) + 1);
    expect(fs.closeSync).toHaveBeenCalledWith(123);
  });

  test.each([undefined, null, {}, "", "postgres.json", "~/postgres.json", `file:///operator/${SECRET}`, `/operator/${SECRET}\0`])(
    "rejects non-explicit absolute file %# before I/O", (file) => {
      expectInvalid(() => loadPostgresLambdaConfig(file, "dev"), "file");
      expect(fs.openSync).not.toHaveBeenCalled();
    },
  );

  test("rejects ephemeral before I/O", () => {
    expectInvalid(() => loadPostgresLambdaConfig(FILE, "ephemeral"), "stage");
    expect(fs.openSync).not.toHaveBeenCalled();
  });

  test.each(["openSync", "fstatSync", "readSync", "closeSync"] as const)("redacts %s failures and causes", (operation) => {
    jest.mocked(fs[operation]).mockImplementationOnce(() => { throw new Error(SECRET, { cause: new Error(FILE) }); });
    expectInvalid(() => loadPostgresLambdaConfig(FILE, "dev"), "file");
    if (operation !== "openSync") expect(fs.closeSync).toHaveBeenCalledWith(123);
  });

  test.each(["", `{\"${SECRET}\":`, `{"password":"${SECRET}",}`, SECRET])("redacts JSON error %#", (contents) => {
    data = Buffer.from(contents);
    expectInvalid(() => loadPostgresLambdaConfig(FILE, "dev"), "file JSON");
    expect(fs.closeSync).toHaveBeenCalledWith(123);
  });

  test("preserves sanitized validation errors after valid JSON", () => {
    data = Buffer.from(JSON.stringify({ ...fixture(), databaseHostname: `postgres://${SECRET}@example.test/db` }));
    expectInvalid(() => loadPostgresLambdaConfig(FILE, "dev"), "databaseHostname");
    expect(fs.closeSync).toHaveBeenCalledWith(123);
  });

  test("requires file stage to match application stage", () => {
    expectInvalid(() => loadPostgresLambdaConfig(FILE, "prod"), "stage");
  });

  test("rejects non-regular files before reading", () => {
    regular = false;
    expectInvalid(() => loadPostgresLambdaConfig(FILE, "dev"), "file");
    expect(fs.readSync).not.toHaveBeenCalled();
    expect(fs.closeSync).toHaveBeenCalledWith(123);
  });

  test("rejects oversize file before allocation/read", () => {
    reportedSize = MAX_BYTES + 1;
    expectInvalid(() => loadPostgresLambdaConfig(FILE, "dev"), "65536");
    expect(fs.readSync).not.toHaveBeenCalled();
    expect(fs.closeSync).toHaveBeenCalledWith(123);
  });

  test("accepts exactly 64 KiB", () => {
    data = Buffer.from(data.toString().padEnd(MAX_BYTES, " "));
    expect(loadPostgresLambdaConfig(FILE, "dev")).toEqual(fixture());
  });

  test("bounds read when file grows after fstat", () => {
    reportedSize = data.length;
    data = Buffer.alloc(MAX_BYTES * 2, " ");
    expectInvalid(() => loadPostgresLambdaConfig(FILE, "dev"), "65536");
    expect(cursor).toBe(MAX_BYTES + 1);
    expect(fs.closeSync).toHaveBeenCalledWith(123);
  });

  test("rejects malformed UTF-8 without exposing bytes", () => {
    data = Buffer.from([0xff, 0xfe]);
    expectInvalid(() => loadPostgresLambdaConfig(FILE, "dev"), "UTF-8");
    expect(fs.closeSync).toHaveBeenCalledWith(123);
  });
});
