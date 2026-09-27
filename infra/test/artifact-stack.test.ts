import * as cdk from "aws-cdk-lib";
import { Match, Template } from "aws-cdk-lib/assertions";
import { PeriodicMatcherArtifactStack } from "../src/artifact-stack";

test("stage-neutral immutable matcher artifact owner has no compute or image dependency", () => {
  const app = new cdk.App();
  const template = Template.fromStack(new PeriodicMatcherArtifactStack(app, "aura-historia-periodic-matcher-artifacts"));
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
