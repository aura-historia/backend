import * as cdk from "aws-cdk-lib";
import { Template } from "aws-cdk-lib/assertions";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { ApplicationEphemeralStack } from "../src/application-stack";

interface EphemeralArtifact {
  readonly binary: string;
  readonly postgres: boolean;
}

interface CloudFormationResource {
  readonly Properties: Record<string, unknown>;
}

const repositoryRoot = resolve(__dirname, "../..");
const artifactBucket = "aura-historia-binary-artifacts-eu-central-1";

function readJson<T>(path: string): T {
  return JSON.parse(readFileSync(resolve(repositoryRoot, path), "utf8")) as T;
}

describe("ephemeral CloudFormation Lambda artifacts", () => {
  test("packages every synthesized artifact and marks PostgreSQL fixture CA consumers", () => {
    const app = new cdk.App({ analyticsReporting: false });
    const stack = new ApplicationEphemeralStack(app, "application-ephemeral", { stage: "ephemeral" });
    const template = Template.fromStack(stack);
    const catalog = readJson<EphemeralArtifact[]>("ci/ephemeral-lambda-binaries.json");
    const releaseBinaries = readJson<string[]>("ci/lambda-binaries.json");
    const synthesized = Object.values(template.findResources("AWS::Lambda::Function"))
      .map((resource) => (resource as CloudFormationResource).Properties)
      .filter((properties) => {
        const code = properties.Code as Record<string, unknown>;
        return code.S3Bucket === artifactBucket;
      })
      .map((properties) => {
        const code = properties.Code as Record<string, unknown>;
        const key = code.S3Key as { "Fn::Join"?: [string, unknown[]] };
        const prefix = key["Fn::Join"]?.[1][0];
        expect(typeof prefix).toBe("string");
        const binary = (prefix as string).match(/^([a-z][a-z0-9-]+)-ephemeral-$/)?.[1];
        expect(binary).toBeDefined();
        const environment = properties.Environment as { Variables?: Record<string, string> } | undefined;
        return {
          binary,
          postgres: environment?.Variables?.POSTGRES_TLS_ROOT_CERT !== undefined,
        };
      }) as EphemeralArtifact[];

    const sortByBinary = (artifacts: EphemeralArtifact[]) =>
      [...artifacts].sort((left, right) => left.binary.localeCompare(right.binary));

    expect(sortByBinary(catalog)).toEqual(sortByBinary(synthesized));
    expect(catalog.every(({ binary }) => releaseBinaries.includes(binary))).toBe(true);
    expect(new Set(catalog.map(({ binary }) => binary)).size).toBe(catalog.length);
  });
});
