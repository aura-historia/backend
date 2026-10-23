import * as cdk from "aws-cdk-lib";
import * as lambda from "aws-cdk-lib/aws-lambda";
import type * as s3 from "aws-cdk-lib/aws-s3";
import { Construct } from "constructs";

export interface LambdaArtifactReference {
  readonly key: string;
  readonly versionId: string;
  readonly sha256: string;
}

const references = new WeakMap<Construct, Map<string, LambdaArtifactReference>>();

/** Parameters identify immutable shared bytes, independently of the deployment target. */
export function lambdaArtifactParameters(scope: Construct, binary: string): LambdaArtifactReference {
  const existing = references.get(scope)?.get(binary);
  if (existing) return existing;
  const id = artifactParameterStem(binary);
  const reference = {
    key: new cdk.CfnParameter(scope, `${id}ArtifactKey`, {
      type: "String", allowedPattern: "^objects/sha256/[a-f0-9]{64}$",
      description: `Completed release bundle S3 object for ${binary}.`,
    }).valueAsString,
    versionId: new cdk.CfnParameter(scope, `${id}ArtifactVersion`, {
      type: "String", minLength: 1,
      description: `Pinned S3 object version for ${binary}.`,
    }).valueAsString,
    sha256: new cdk.CfnParameter(scope, `${id}ArtifactSha256`, {
      type: "String", allowedPattern: "^[a-f0-9]{64}$",
      description: `Content SHA256 binding for the ${binary} immutable version.`,
    }).valueAsString,
  };
  const values = references.get(scope) ?? new Map();
  values.set(binary, reference);
  references.set(scope, values);
  return reference;
}

export function artifactParameterStem(binary: string): string {
  return binary.split("-").map((part) => part[0].toUpperCase() + part.slice(1)).join("");
}

export function releaseLambdaCode(scope: Construct, bucket: s3.IBucket, binary: string): lambda.Code {
  const artifact = lambdaArtifactParameters(scope, binary);
  return lambda.Code.fromBucket(bucket, artifact.key, artifact.versionId);
}

export function routineStackNames(stage: "dev" | "prod"): readonly string[] {
  return ["migrate", "compute", "api", ...(stage === "prod" ? ["observability"] : [])]
    .map((boundary) => `application-${stage}-${boundary}`);
}
