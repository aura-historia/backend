import * as cdk from "aws-cdk-lib";
import * as iam from "aws-cdk-lib/aws-iam";

import { Construct } from "constructs";
import type { StageConfig } from "../config";

export interface SearchProps {
  readonly config: StageConfig;
}

export class Search extends Construct {
  readonly endpointUrl: string;
  readonly domainName: string;
  readonly domainArnForIam: string;

  constructor(scope: Construct, id: string, props: SearchProps) {
    super(scope, id);

    this.domainName = props.config.opensearchDomainName;
    this.endpointUrl = props.config.opensearchEndpointUrl;
    this.domainArnForIam = cdk.Stack.of(this).formatArn({
      service: "es",
      resource: "domain",
      resourceName: `${this.domainName}/*`,
    });
  }

  grantRead(grantee: iam.IGrantable): void {
    this.addPolicy(grantee, ["es:Describe*", "es:List*", "es:ESHttpGet", "es:ESHttpHead", "es:ESHttpPost"]);
  }

  grantIndexDocumentWrite(grantee: iam.IGrantable): void {
    this.addPolicy(grantee, ["es:ESHttpPut"]);
  }

  private addPolicy(grantee: iam.IGrantable, actions: string[]): void {
    grantee.grantPrincipal.addToPrincipalPolicy(
      new iam.PolicyStatement({
        actions,
        resources: [this.domainArnForIam],
      }),
    );
  }
}
