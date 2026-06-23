import * as cdk from "aws-cdk-lib";
import * as iam from "aws-cdk-lib/aws-iam";
import * as opensearch from "aws-cdk-lib/aws-opensearchservice";
import { Construct } from "constructs";
import type { StageConfig } from "../config";

export interface SearchProps {
  readonly config: StageConfig;
  readonly externalEndpointUrl: string;
}

export class Search extends Construct {
  readonly endpointUrl: string;
  readonly domainName: string;
  readonly domainArnForIam: string;
  readonly domain: opensearch.CfnDomain | undefined;

  constructor(scope: Construct, id: string, props: SearchProps) {
    super(scope, id);

    this.domainName = props.config.opensearchDomainName;

    if (props.config.isEphemeral) {
      this.domain = new opensearch.CfnDomain(this, "OpenSearchDomain", {
        domainName: this.domainName,
        engineVersion: "OpenSearch_3.1",
        clusterConfig: {
          instanceCount: 1,
          instanceType: "t3.small.search",
          zoneAwarenessEnabled: false,
        },
        ebsOptions: {
          ebsEnabled: true,
          volumeSize: 10,
          volumeType: "gp3",
        },
        domainEndpointOptions: {
          customEndpoint: "http://localhost:4566/test-domain",
          customEndpointEnabled: true,
        },
      });

      this.endpointUrl = cdk.Fn.sub("https://${DomainEndpoint}", {
        DomainEndpoint: this.domain.attrDomainEndpoint,
      });
      this.domainArnForIam = cdk.Fn.sub("${DomainArn}/*", {
        DomainArn: this.domain.attrArn,
      });
    } else {
      this.endpointUrl = props.externalEndpointUrl;
      this.domainArnForIam = cdk.Stack.of(this).formatArn({
        service: "es",
        resource: "domain",
        resourceName: `${this.domainName}/*`,
      });
    }
  }

  grantRead(grantee: iam.IGrantable): void {
    this.addPolicy(grantee, ["es:Describe*", "es:List*", "es:ESHttpGet", "es:ESHttpHead", "es:ESHttpPost"]);
  }

  grantReadWrite(grantee: iam.IGrantable): void {
    this.addPolicy(grantee, [
      "es:Describe*",
      "es:List*",
      "es:ESHttpGet",
      "es:ESHttpHead",
      "es:ESHttpPost",
      "es:ESHttpPut",
      "es:ESHttpDelete",
    ]);
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
