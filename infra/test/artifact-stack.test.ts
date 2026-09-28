import { spawnSync } from "node:child_process";
import { mkdtempSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import * as cdk from "aws-cdk-lib";
import { Match, Template } from "aws-cdk-lib/assertions";
import { ContainerArtifactStack } from "../src/artifact-stack";
import { CONTAINER_IMAGE_CATALOG } from "../src/container-image-catalog";
import { CLOUDFORMATION_STAGING_BUCKET_NAME, WORKLOAD_REGION } from "../src/config";

test("stage-neutral immutable container artifact owner has no compute or image dependency", () => {
  const app = new cdk.App();
  const template = Template.fromStack(new ContainerArtifactStack(app, "aura-historia-container-artifacts"));
  template.resourceCountIs("AWS::ECR::Repository", 1);
  template.hasResource("AWS::ECR::Repository", {
    DeletionPolicy: "Retain",
    Properties: {
      RepositoryName: "aura-historia-periodic-matcher",
      ImageTagMutability: "IMMUTABLE",
      ImageScanningConfiguration: { ScanOnPush: true },
      EncryptionConfiguration: { EncryptionType: "AES256" },
      LifecyclePolicy: Match.absent(),
    },
  });
  expect(Object.keys(template.toJSON().Parameters ?? {}).filter((name) => name !== "BootstrapVersion")).toEqual([]);
  template.resourceCountIs("AWS::ECS::TaskDefinition", 0);
});

test("artifact app publishes its CloudFormation template to the shared staging bucket", () => {
  const output = mkdtempSync(join(tmpdir(), "container-artifacts-synth-"));
  try {
    const stackName = "aura-historia-container-artifacts";
    const result = spawnSync(resolve(process.cwd(), "node_modules/.bin/cdk"), [
      "synth", stackName,
      "--app", "npx ts-node --prefer-ts-exts bin/artifacts.ts",
      "--output", output,
      "-c", "account=123456789012",
    ], { cwd: process.cwd(), encoding: "utf8", timeout: 60_000 });
    expect(result.error).toBeUndefined();
    if (result.status !== 0) throw new Error(`CDK synth failed: ${result.stderr}`);
    const assets = JSON.parse(readFileSync(join(output, `${stackName}.assets.json`), "utf8")) as {
      files: Record<string, { destinations: Record<string, { bucketName: string; region: string }> }>;
    };
    const destinations = Object.values(assets.files).flatMap((asset) => Object.values(asset.destinations));
    expect(destinations).not.toHaveLength(0);
    expect(destinations).toEqual(expect.arrayContaining([
      expect.objectContaining({ bucketName: CLOUDFORMATION_STAGING_BUCKET_NAME, region: WORKLOAD_REGION }),
    ]));
    expect(destinations.every((destination) => destination.bucketName === CLOUDFORMATION_STAGING_BUCKET_NAME)).toBe(true);
  } finally {
    rmSync(output, { recursive: true, force: true });
  }
}, 90_000);

test("catalog repositories coexist without identity collisions", () => {
  const app = new cdk.App();
  const matcher = CONTAINER_IMAGE_CATALOG[0];
  const fixture = {
    ...matcher,
    id: "secondary-worker",
    repository: "aura-historia-secondary-worker",
    digestParameter: "SecondaryWorkerImageDigest",
    taskDefinitionOutput: "SecondaryWorkerTaskDefinitionArn",
  };
  const template = Template.fromStack(new ContainerArtifactStack(app, "two-image-artifacts", {
    images: [matcher, fixture],
  }));
  const repositories = Object.values(template.findResources("AWS::ECR::Repository"));
  expect(repositories).toHaveLength(2);
  expect(repositories.map((resource) => resource.Properties.RepositoryName).sort()).toEqual([
    "aura-historia-periodic-matcher",
    "aura-historia-secondary-worker",
  ]);
  expect(Object.keys(template.toJSON().Outputs ?? {}).sort()).toEqual([
    "PeriodicMatcherRepositoryArn",
    "PeriodicMatcherRepositoryUri",
    "SecondaryWorkerRepositoryArn",
    "SecondaryWorkerRepositoryUri",
  ]);
});
