import * as cdk from "aws-cdk-lib";
import * as iam from "aws-cdk-lib/aws-iam";
import * as lambda from "aws-cdk-lib/aws-lambda";
import * as s3 from "aws-cdk-lib/aws-s3";
import { Construct } from "constructs";
import { ARTIFACT_BUCKET_NAME, type StageConfig } from "../config";
import { lambdaArtifactParameters, releaseLambdaCode } from "../release-artifacts";
import { LAMBDA_LOG_LEVEL, LAMBDA_LOGGING, lambdaLogGroup } from "./lambda-logging";

/** Cognito invokes the stable alias, so a code update publishes a version before it takes effect. */
export const COGNITO_TRIGGER_ALIAS_NAME = "live";

interface TriggerDefinition {
  readonly id: string;
  readonly binaryName: string;
  /** Distinct from the retired VPC triggers, which keep their names until their stack is deleted. */
  readonly functionName: (stage: string) => string;
  readonly environment: (config: StageConfig) => Record<string, string>;
}

export const PRE_SIGN_UP_TRIGGER: TriggerDefinition = {
  id: "PreSignUp",
  binaryName: "cognito-pre-sign-up",
  functionName: (stage) => `cognito-pre-sign-up-trigger-${stage}`,
  environment: (config) => ({ COGNITO_PROVIDER_SIGNUP_POLICY: providerSignupPolicy(config) }),
};

/** Forwards each confirmation to the host's signed internal endpoint; it holds no business state. */
export const POST_CONFIRMATION_TRIGGER: TriggerDefinition = {
  id: "PostConfirmation",
  binaryName: "cognito-post-confirmation-forwarder",
  functionName: (stage) => `cognito-post-confirmation-forwarder-${stage}`,
  environment: () => ({}),
};

export interface CognitoTriggersProps {
  readonly config: StageConfig;
}

/**
 * The two Lambda functions Cognito invokes. They run outside any VPC: the host's database is
 * private, so each reaches only Cognito or the host's public front door.
 */
export class CognitoTriggers extends Construct {
  readonly preSignUp: lambda.Alias;
  readonly postConfirmation: lambda.Alias;

  constructor(scope: Construct, id: string, props: CognitoTriggersProps) {
    super(scope, id);
    const artifactBucket = s3.Bucket.fromBucketName(this, "ArtifactBucketImport", ARTIFACT_BUCKET_NAME);
    this.preSignUp = triggerFunction(this, props.config, artifactBucket, PRE_SIGN_UP_TRIGGER);
    this.postConfirmation = triggerFunction(this, props.config, artifactBucket, POST_CONFIRMATION_TRIGGER);
    grantFederatedLinking(this.preSignUp);
  }
}

function triggerFunction(
  scope: Construct,
  config: StageConfig,
  artifactBucket: s3.IBucket,
  definition: TriggerDefinition,
): lambda.Alias {
  const stack = cdk.Stack.of(scope);
  const functionName = definition.functionName(config.stage);
  const fn = new lambda.Function(scope, `${definition.id}Function`, {
    functionName,
    runtime: lambda.Runtime.PROVIDED_AL2023,
    architecture: lambda.Architecture.X86_64,
    handler: "lib.handler",
    code: releaseLambdaCode(stack, artifactBucket, definition.binaryName),
    currentVersionOptions: { description: lambdaArtifactParameters(stack, definition.binaryName).sha256 },
    memorySize: 256,
    // Cognito waits at most five seconds for a trigger.
    timeout: cdk.Duration.seconds(5),
    reservedConcurrentExecutions: 5,
    logGroup: lambdaLogGroup(scope, `${definition.id}LogGroup`, functionName, config),
    ...LAMBDA_LOGGING,
    environment: { LOG_LEVEL: LAMBDA_LOG_LEVEL, ...definition.environment(config) },
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
