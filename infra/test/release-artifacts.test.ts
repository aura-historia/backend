import * as cdk from "aws-cdk-lib";
import { Template } from "aws-cdk-lib/assertions";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { createApplicationStacks } from "../src/application-stack";
import { STAGES } from "../src/config";

const releaseBinaries = JSON.parse(readFileSync(resolve(__dirname, "../../ci/lambda-binaries.json"), "utf8")) as string[];
const artifactBucket = "aura-historia-release-bundles-eu-central-1";

test("does not publish removed saved-filter matching or projection binaries", () => {
  expect(releaseBinaries).not.toContain("search-filter-percolator-lambda");
  expect(releaseBinaries).not.toContain("search-filter-projection-lambda");
});

describe.each(STAGES)("%s release Lambda artifacts", (stage) => {
  test("every synthesized Lambda has a published binary", () => {
    const app = new cdk.App({ analyticsReporting: false });
    const { compute, initialization, migration } = createApplicationStacks(app, { stage });
    const keys = new Set<string>();
    for (const stack of [compute, initialization, migration]) {
      const template = Template.fromStack(stack);
      for (const resource of Object.values(template.findResources("AWS::Lambda::Function"))) {
        const code = resource.Properties.Code;
        if (!code.S3Bucket) continue;
        expect(code.S3Bucket).toBe(artifactBucket);
        expect(code.S3Key.Ref).toMatch(/ArtifactKey$/);
        expect(code.S3ObjectVersion.Ref).toMatch(/ArtifactVersion$/);
        keys.add(code.S3Key.Ref);
      }
    }
    const stem = (binary: string) => binary.split("-").map((part) => part[0].toUpperCase() + part.slice(1)).join("");
    expect([...keys].sort()).toEqual(releaseBinaries.map((binary) => `${stem(binary)}ArtifactKey`).sort());
  });
});
