import * as cdk from "aws-cdk-lib";
import { Construct } from "constructs";

export interface ApplicationParameters {
  readonly commitSha: string;
}

export function applicationParameters(scope: Construct): ApplicationParameters {
  const commitSha = new cdk.CfnParameter(scope, "CommitSHA", {
    type: "String",
    description: "Artifact version to deploy. Reusing an older SHA rolls back Lambda/template artifacts.",
  });

  return {
    commitSha: commitSha.valueAsString,
  };
}
