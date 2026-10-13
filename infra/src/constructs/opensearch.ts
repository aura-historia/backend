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

  constructor(scope: Construct, id: string, props: SearchProps) {
    super(scope, id);

    this.domainName = props.config.opensearchDomainName;
    this.endpointUrl = props.config.opensearchEndpointUrl;
  }

  grantIndexDocumentWrite(grantee: iam.IGrantable, index: string): void {
    grantee.grantPrincipal.addToPrincipalPolicy(
      new iam.PolicyStatement({
        actions: ["es:ESHttpPost"],
        resources: [cdk.Stack.of(this).formatArn({
          service: "es",
          resource: "domain",
          resourceName: `${this.domainName}/${index}/_doc/*`,
        })],
      }),
    );
  }
}
