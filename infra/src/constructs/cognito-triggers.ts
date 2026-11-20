import * as cdk from "aws-cdk-lib";
import * as iam from "aws-cdk-lib/aws-iam";
import * as lambda from "aws-cdk-lib/aws-lambda";
import * as s3 from "aws-cdk-lib/aws-s3";
import { Construct } from "constructs";
import { LAMBDA_ARTIFACT_BUCKET_NAME, type StageConfig } from "../config";
import { credentialSecrets, credentialValue } from "../credentials";
import { LAMBDA_LOG_LEVEL, LAMBDA_LOGGING, lambdaLogGroup } from "./lambda-logging";

/** Cognito invokes the stable alias, so a code update publishes a version before it takes effect. */
export const COGNITO_TRIGGER_ALIAS_NAME = "live";

/** The object the Deploy workflow publishes for one trigger binary built from one commit. */
export function triggerArtifactKey(binaryName: string, stage: string, commit: string): string {
  return `${binaryName}-${stage}-${commit}.zip`;
}

interface TriggerDefinition {
  readonly id: string;
  readonly binaryName: string;
  /** Distinct from the retired VPC triggers, which keep their names until their stack is deleted. */
  readonly functionName: (stage: string) => string;
  readonly environment: (scope: Construct, config: StageConfig) => Record<string, string>;
}

/** The host's internal endpoint the post-confirmation forwarder calls (`aura-historia-api`). */
export const POST_CONFIRMATION_ENDPOINT_PATH = "/internal/cognito/post-confirmation";
/** The host's internal endpoint the pre-sign-up trigger asks for federated decisions. */
export const PRE_SIGN_UP_ENDPOINT_PATH = "/internal/cognito/pre-sign-up";

/**
 * Applies the provider policy and links federated identities itself, but asks the host's internal
 * endpoint, through the public API hostname, whether the sign-up's email already has an account.
 * It signs with the post-confirmation forwarder's key under its own purpose label.
 */
export const PRE_SIGN_UP_TRIGGER: TriggerDefinition = {
  id: "PreSignUp",
  binaryName: "cognito-pre-sign-up",
  functionName: (stage) => `cognito-pre-sign-up-trigger-${stage}`,
  environment: (scope, config) => ({
    COGNITO_PROVIDER_SIGNUP_POLICY: providerSignupPolicy(config),
    PRE_SIGN_UP_ENDPOINT_URL: `https://${config.apiDomainName}${PRE_SIGN_UP_ENDPOINT_PATH}`,
    PRE_SIGN_UP_SIGNING_KEY: triggerSigningKey(scope, config),
  }),
};

/**
 * Forwards each confirmation, signed and timestamped, to the host's internal endpoint through the
 * public API hostname, because only CloudFront adds the origin secret the host requires. It holds
 * no business state. The signing key is resolved at deployment from the version-pinned
 * secret the host stack generates.
 */
export const POST_CONFIRMATION_TRIGGER: TriggerDefinition = {
  id: "PostConfirmation",
  binaryName: "cognito-post-confirmation-forwarder",
  functionName: (stage) => `cognito-post-confirmation-forwarder-${stage}`,
  environment: (scope, config) => ({
    POST_CONFIRMATION_ENDPOINT_URL: `https://${config.apiDomainName}${POST_CONFIRMATION_ENDPOINT_PATH}`,
    POST_CONFIRMATION_SIGNING_KEY: triggerSigningKey(scope, config),
  }),
};

/** Both triggers sign their host requests with one key, each under its own purpose label. */
function triggerSigningKey(scope: Construct, config: StageConfig): string {
  return credentialValue(scope, credentialSecrets(config.stage).cognitoTriggerSigning, "value").unsafeUnwrap();
}

export interface CognitoTriggersProps {
  readonly config: StageConfig;
}

/**
 * The two Lambda functions Cognito invokes. They run outside any VPC: the host's database is
 * private, so each reaches only Cognito and the host's public front door.
 */
export class CognitoTriggers extends Construct {
  readonly preSignUp: lambda.Alias;
  readonly postConfirmation: lambda.Alias;

  constructor(scope: Construct, id: string, props: CognitoTriggersProps) {
    super(scope, id);
    const artifactBucket = s3.Bucket.fromBucketName(this, "ArtifactBucketImport", LAMBDA_ARTIFACT_BUCKET_NAME);
    // A commit the Deploy workflow deployed, which published both triggers' zips for it.
    const revision = new cdk.CfnParameter(cdk.Stack.of(this), "CognitoTriggersRevision", {
      type: "String",
      allowedPattern: "^[0-9a-f]{40}$",
      description: "Commit whose Cognito trigger zips the Deploy workflow published.",
    }).valueAsString;
    this.preSignUp = triggerFunction(this, props.config, artifactBucket, revision, PRE_SIGN_UP_TRIGGER);
    this.postConfirmation = triggerFunction(this, props.config, artifactBucket, revision, POST_CONFIRMATION_TRIGGER);
    grantFederatedLinking(this.preSignUp);
  }
}

function triggerFunction(
  scope: Construct,
  config: StageConfig,
  artifactBucket: s3.IBucket,
  revision: string,
  definition: TriggerDefinition,
): lambda.Alias {
  const functionName = definition.functionName(config.stage);
  const fn = new lambda.Function(scope, `${definition.id}Function`, {
    functionName,
    runtime: lambda.Runtime.PROVIDED_AL2023,
    architecture: lambda.Architecture.X86_64,
    handler: "lib.handler",
    code: lambda.Code.fromBucket(artifactBucket, triggerArtifactKey(definition.binaryName, config.stage, revision)),
    // A new commit publishes a new version, which the alias then points to.
    currentVersionOptions: { description: revision },
    memorySize: 256,
    // Cognito waits at most five seconds for a trigger.
    timeout: cdk.Duration.seconds(5),
    reservedConcurrentExecutions: 5,
    logGroup: lambdaLogGroup(scope, `${definition.id}LogGroup`, functionName, config),
    ...LAMBDA_LOGGING,
    environment: { LOG_LEVEL: LAMBDA_LOG_LEVEL, ...definition.environment(scope, config) },
  });
  return new lambda.Alias(scope, `${definition.id}Alias`, {
    aliasName: COGNITO_TRIGGER_ALIAS_NAME,
    version: fn.currentVersion,
  });
}

/** Per-provider collision policy for federated sign-ups, read by the pre-sign-up trigger. */
function providerSignupPolicy(config: StageConfig): string {
  return JSON.stringify(config.cognitoIdentityProviders.map((provider) => ({
    providerName: provider.providerName,
    existingEmailAction: provider.existingEmailAction,
    ...(provider.existingEmailAction === "LINK_VERIFIED"
      ? { linkSourceAttributeName: provider.linkSourceAttributeName }
      : {}),
  })));
}

/**
 * The pool invokes this function, so its ARN cannot name the pool without a dependency cycle;
 * the grant covers the account's pools in the workload region instead.
 */
function grantFederatedLinking(preSignUp: lambda.Alias): void {
  preSignUp.lambda.addToRolePolicy(new iam.PolicyStatement({
    actions: ["cognito-idp:ListUsers", "cognito-idp:AdminLinkProviderForUser"],
    resources: [cdk.Stack.of(preSignUp).formatArn({
      service: "cognito-idp",
      resource: "userpool",
      resourceName: "*",
      arnFormat: cdk.ArnFormat.SLASH_RESOURCE_NAME,
    })],
  }));
}
