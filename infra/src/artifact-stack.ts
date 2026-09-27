import * as cdk from "aws-cdk-lib";
import * as ecr from "aws-cdk-lib/aws-ecr";
import { Construct } from "constructs";
import { PERIODIC_MATCHER_REPOSITORY } from "./periodic-matcher-config";

export class PeriodicMatcherArtifactStack extends cdk.Stack {
  constructor(scope: Construct, id: string, props?: cdk.StackProps) {
    super(scope, id, props);
    const repository = new ecr.Repository(this, "PeriodicMatcherRepository", {
      repositoryName: PERIODIC_MATCHER_REPOSITORY,
      imageTagMutability: ecr.TagMutability.IMMUTABLE,
      imageScanOnPush: true,
      encryption: ecr.RepositoryEncryption.AES_256,
      removalPolicy: cdk.RemovalPolicy.RETAIN,
      emptyOnDelete: false,
    });
    // Declare the default encryption explicitly so drift inspection can verify it.
    (repository.node.defaultChild as ecr.CfnRepository).encryptionConfiguration = { encryptionType: "AES256" };
    new cdk.CfnOutput(this, "PeriodicMatcherRepositoryUri", {
      value: repository.repositoryUri,
      exportName: "AuraHistoriaPeriodicMatcherRepositoryUri",
    });
    new cdk.CfnOutput(this, "PeriodicMatcherRepositoryArn", { value: repository.repositoryArn });
  }
}
