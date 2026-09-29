import * as cdk from "aws-cdk-lib";
import * as ecr from "aws-cdk-lib/aws-ecr";
import { Construct } from "constructs";
import { CONTAINER_IMAGE_CATALOG, type ContainerImage } from "./container-image-catalog";

export interface ContainerArtifactStackProps extends cdk.StackProps {
  readonly images?: readonly ContainerImage[];
}

export class ContainerArtifactStack extends cdk.Stack {
  constructor(scope: Construct, id: string, props: ContainerArtifactStackProps = {}) {
    const { images = CONTAINER_IMAGE_CATALOG, ...stackProps } = props;
    super(scope, id, stackProps);
    if (images.length === 0) throw new Error("Container artifact stack requires at least one image repository.");
    const names = new Set<string>();
    const ids = new Set<string>();
    for (const image of images) {
      const stem = image.id.split("-").map((part) => `${part[0].toUpperCase()}${part.slice(1)}`).join("");
      if (ids.has(image.id) || names.has(image.repository) || names.has(stem)) {
        throw new Error(`Container artifact catalog has a duplicate repository identity for '${image.id}'.`);
      }
      ids.add(image.id);
      names.add(image.repository);
      names.add(stem);
      const repository = new ecr.Repository(this, `${stem}Repository`, {
        repositoryName: image.repository,
        imageTagMutability: ecr.TagMutability.IMMUTABLE,
        imageScanOnPush: true,
        encryption: ecr.RepositoryEncryption.AES_256,
        removalPolicy: cdk.RemovalPolicy.RETAIN,
        emptyOnDelete: false,
      });
      // Declare the default encryption explicitly so drift inspection can verify it.
      (repository.node.defaultChild as ecr.CfnRepository).encryptionConfiguration = { encryptionType: "AES256" };
      new cdk.CfnOutput(this, `${stem}RepositoryUri`, {
        value: repository.repositoryUri,
        exportName: `AuraHistoria${stem}RepositoryUri`,
      });
      new cdk.CfnOutput(this, `${stem}RepositoryArn`, { value: repository.repositoryArn });
    }
  }
}
