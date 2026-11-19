import * as cdk from "aws-cdk-lib";
import * as iam from "aws-cdk-lib/aws-iam";
import { Construct } from "constructs";
import { LAMBDA_ARTIFACT_BUCKET_NAME, type StageConfig } from "../config";
import { triggerArtifactKey } from "./cognito-triggers";

/** The repository whose Deploy workflow publishes the Cognito trigger code. */
export const GITHUB_REPOSITORY = "aura-historia/backend";
/** The protected GitHub environment whose jobs may assume the publisher role. */
export const GITHUB_DEPLOY_ENVIRONMENT = "prod";

export interface TriggerArtifactPublisherProps {
  readonly config: StageConfig;
}

/**
 * The role the Deploy workflow's approved `prod` job assumes through GitHub's OIDC provider to
 * publish the Cognito trigger zips of the commit it deploys. It may only create objects named like
 * trigger artifacts; it cannot read, replace, delete or deploy anything.
 */
export class TriggerArtifactPublisher extends Construct {
  readonly role: iam.Role;

  constructor(scope: Construct, id: string, props: TriggerArtifactPublisherProps) {
    super(scope, id);
    const stack = cdk.Stack.of(this);
    // The account's GitHub OIDC provider predates this app; it is shared, so it is imported.
    const provider = iam.OpenIdConnectProvider.fromOpenIdConnectProviderArn(this, "GitHubOidcProvider",
      stack.formatArn({ service: "iam", region: "", resource: "oidc-provider", resourceName: "token.actions.githubusercontent.com" }));
    this.role = new iam.Role(this, "Role", {
      roleName: `aura-historia-${props.config.stage}-trigger-artifact-publisher`,
      description: "Publishes the Cognito trigger zips from the Deploy workflow's prod environment.",
      maxSessionDuration: cdk.Duration.hours(1),
      assumedBy: new iam.WebIdentityPrincipal(provider.openIdConnectProviderArn, {
        StringEquals: {
          "token.actions.githubusercontent.com:aud": "sts.amazonaws.com",
          "token.actions.githubusercontent.com:sub": `repo:${GITHUB_REPOSITORY}:environment:${GITHUB_DEPLOY_ENVIRONMENT}`,
        },
      }),
    });
    // Creation only: a write without If-None-Match, which could replace a published zip, is denied.
    const keys = triggerArtifactKey("cognito-*", props.config.stage, "*");
    this.role.addToPolicy(new iam.PolicyStatement({
      actions: ["s3:PutObject"],
      resources: [`arn:${stack.partition}:s3:::${LAMBDA_ARTIFACT_BUCKET_NAME}/${keys}`],
      conditions: { StringEquals: { "s3:if-none-match": "*" } },
    }));
  }
}
