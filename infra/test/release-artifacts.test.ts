import * as cdk from "aws-cdk-lib";
import { Template } from "aws-cdk-lib/assertions";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { createApplicationStacks } from "../src/application-stack";
import { STAGES } from "../src/config";

const releaseBinaries = JSON.parse(readFileSync(resolve(__dirname, "../../ci/lambda-binaries.json"), "utf8")) as string[];
const artifactBucket = "aura-historia-binary-artifacts-eu-central-1";

describe.each(STAGES)("%s release Lambda artifacts", (stage) => {
  test("every synthesized Lambda has a published binary", () => {
    const app = new cdk.App({ analyticsReporting: false });
    const { compute, initialization } = createApplicationStacks(app, { stage });
    const binaries = [compute, initialization].flatMap((stack) =>
      Object.values(Template.fromStack(stack).findResources("AWS::Lambda::Function")).map((resource) => {
        const code = resource.Properties.Code as {
          S3Bucket: string;
          S3Key: { "Fn::Join": [string, unknown[]] };
        };
        expect(code.S3Bucket).toBe(artifactBucket);
        const prefix = code.S3Key["Fn::Join"][1][0];
        expect(typeof prefix).toBe("string");
        const binary = (prefix as string).match(new RegExp(`^([a-z][a-z0-9-]+)-${stage}-$`))?.[1];
        expect(binary).toBeDefined();
        return binary;
      }),
    );

    expect(binaries.sort()).toEqual([...releaseBinaries].sort());
    expect(new Set(binaries).size).toBe(binaries.length);
  });
});
