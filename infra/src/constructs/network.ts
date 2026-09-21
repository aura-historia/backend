import * as ec2 from "aws-cdk-lib/aws-ec2";
import * as iam from "aws-cdk-lib/aws-iam";
import { Construct } from "constructs";
import {
  ARTIFACT_BUCKET_NAME,
  CLOUDFORMATION_STAGING_BUCKET_NAME,
  MAIL_TEMPLATE_BUCKET_NAME,
  type StageConfig,
} from "../config";

export interface NetworkProps {
  readonly config: StageConfig;
}

export class Network extends Construct {
  readonly vpc: ec2.IVpc;
  readonly applicationSecurityGroup: ec2.ISecurityGroup;
  readonly databaseSecurityGroup: ec2.ISecurityGroup;
  readonly dmsSecurityGroup: ec2.ISecurityGroup;
  readonly dmsEndpointSecurityGroup: ec2.ISecurityGroup;
  readonly migrationSecurityGroup: ec2.ISecurityGroup;
  readonly natEip: ec2.CfnEIP;

  constructor(scope: Construct, id: string, props: NetworkProps) {
    super(scope, id);

    if (!props.config.network) {
      throw new Error("Network resources are only available in real AWS stages.");
    }

    this.natEip = new ec2.CfnEIP(this, "NatGatewayEip", {
      domain: "vpc",
      tags: [{ key: "Name", value: `aura-historia-${props.config.stage}-nat-eip` }],
    });
    this.natEip.applyRemovalPolicy(props.config.removalPolicy);

    this.vpc = new ec2.Vpc(this, "Vpc", {
      ipAddresses: ec2.IpAddresses.cidr(props.config.network.cidr),
      maxAzs: 2,
      natGateways: 1,
      natGatewayProvider: ec2.NatProvider.gateway({
        eipAllocationIds: [this.natEip.attrAllocationId],
      }),
      subnetConfiguration: [
        { name: "public", subnetType: ec2.SubnetType.PUBLIC, cidrMask: 24 },
        { name: "application", subnetType: ec2.SubnetType.PRIVATE_WITH_EGRESS, cidrMask: 24 },
        { name: "database", subnetType: ec2.SubnetType.PRIVATE_ISOLATED, cidrMask: 24 },
      ],
      vpcName: `aura-historia-${props.config.stage}`,
    });

    const s3Endpoint = this.vpc.addGatewayEndpoint("S3GatewayEndpoint", {
      service: ec2.GatewayVpcEndpointAwsService.S3,
      subnets: [{ subnetType: ec2.SubnetType.PRIVATE_WITH_EGRESS }],
    });
    s3Endpoint.addToPolicy(new iam.PolicyStatement({
      actions: ["s3:GetObject", "s3:ListBucket"],
      principals: [new iam.AnyPrincipal()],
      resources: s3EndpointResources(),
    }));

    this.applicationSecurityGroup = new ec2.SecurityGroup(this, "ApplicationSecurityGroup", {
      vpc: this.vpc,
      allowAllOutbound: false,
      description: "Backend application workloads in private application subnets",
      securityGroupName: `aura-historia-application-${props.config.stage}`,
    });
    this.databaseSecurityGroup = new ec2.SecurityGroup(this, "DatabaseSecurityGroup", {
      vpc: this.vpc,
      allowAllOutbound: false,
      description: "Private PostgreSQL database boundary",
      securityGroupName: `aura-historia-database-${props.config.stage}`,
    });
    this.dmsSecurityGroup = new ec2.SecurityGroup(this, "DmsSecurityGroup", {
      vpc: this.vpc,
      allowAllOutbound: false,
      description: "Private DMS replication instances",
      securityGroupName: `aura-historia-dms-${props.config.stage}`,
    });
    this.dmsEndpointSecurityGroup = new ec2.SecurityGroup(this, "DmsEndpointSecurityGroup", {
      vpc: this.vpc,
      allowAllOutbound: false,
      description: "DMS interface endpoint boundary",
      securityGroupName: `aura-historia-dms-endpoint-${props.config.stage}`,
    });
    this.migrationSecurityGroup = new ec2.SecurityGroup(this, "MigrationSecurityGroup", {
      vpc: this.vpc,
      allowAllOutbound: false,
      description: "Approved database migration workload boundary",
      securityGroupName: `aura-historia-migration-${props.config.stage}`,
    });

    this.applicationSecurityGroup.addEgressRule(ec2.Peer.anyIpv4(), ec2.Port.tcp(443), "Required HTTPS provider and AWS API egress through NAT");
    this.applicationSecurityGroup.addEgressRule(this.databaseSecurityGroup, ec2.Port.tcp(5432), "Private PostgreSQL");
    this.dmsSecurityGroup.addEgressRule(this.databaseSecurityGroup, ec2.Port.tcp(5432), "Private PostgreSQL replication");
    this.dmsSecurityGroup.addEgressRule(this.dmsEndpointSecurityGroup, ec2.Port.tcp(443), "Private Kinesis and Secrets Manager endpoints");
    this.migrationSecurityGroup.addEgressRule(this.databaseSecurityGroup, ec2.Port.tcp(5432), "Private PostgreSQL migrations");

    this.databaseSecurityGroup.addIngressRule(this.applicationSecurityGroup, ec2.Port.tcp(5432), "Backend application PostgreSQL");
    this.databaseSecurityGroup.addIngressRule(this.dmsSecurityGroup, ec2.Port.tcp(5432), "DMS PostgreSQL replication");
    this.databaseSecurityGroup.addIngressRule(this.migrationSecurityGroup, ec2.Port.tcp(5432), "Approved migrations PostgreSQL");
    this.dmsEndpointSecurityGroup.addIngressRule(this.dmsSecurityGroup, ec2.Port.tcp(443), "DMS AWS API calls");
  }
}

function s3EndpointResources(): string[] {
  return [ARTIFACT_BUCKET_NAME, MAIL_TEMPLATE_BUCKET_NAME, CLOUDFORMATION_STAGING_BUCKET_NAME]
    .flatMap((bucket) => [`arn:aws:s3:::${bucket}`, `arn:aws:s3:::${bucket}/*`]);
}
