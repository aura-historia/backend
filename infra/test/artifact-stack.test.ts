import * as cdk from "aws-cdk-lib";
import { Match, Template } from "aws-cdk-lib/assertions";
import { ContainerArtifactStack } from "../src/artifact-stack";
import { CONTAINER_IMAGE_CATALOG } from "../src/container-image-catalog";

test("stage-neutral immutable matcher artifact owner has no compute or image dependency", () => {
  const app = new cdk.App();
  const template = Template.fromStack(new ContainerArtifactStack(app, "aura-historia-periodic-matcher-artifacts"));
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
