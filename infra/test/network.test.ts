import * as cdk from "aws-cdk-lib";
import { Template } from "aws-cdk-lib/assertions";
import { createApplicationStacks } from "../src/application-stack";
import type { StageName } from "../src/config";

const REAL_STAGES: StageName[] = ["dev", "prod"];

function networkTemplate(stage: StageName): Template {
  const app = new cdk.App({ analyticsReporting: false });
  const stacks = createApplicationStacks(app, { stage });
  expect(stacks.network).toBeDefined();
  return Template.fromStack(stacks.network!);
}

function namedRouteTables(template: Template, subnetKind: string): string[] {
  return Object.entries(template.findResources("AWS::EC2::RouteTable"))
    .filter(([, resource]) => JSON.stringify(resource.Properties.Tags).includes(`/${subnetKind}Subnet`))
    .map(([logicalId]) => logicalId);
}

describe.each(REAL_STAGES)("%s private workload network", (stage) => {
  test("has exactly one explicit EIP and managed NAT across two AZ subnet tiers", () => {
    const template = networkTemplate(stage);
    const eips = template.findResources("AWS::EC2::EIP");
    const nats = template.findResources("AWS::EC2::NatGateway");
    const subnets = template.findResources("AWS::EC2::Subnet");

    expect(Object.values(eips)).toHaveLength(1);
    expect(Object.values(nats)).toHaveLength(1);
    expect(Object.values(subnets)).toHaveLength(6);
    expect(Object.values(subnets).filter((subnet) => subnet.Properties.MapPublicIpOnLaunch === true)).toHaveLength(2);

    const [natId, nat] = Object.entries(nats)[0];
    const [eipId] = Object.keys(eips);
    expect(nat.Properties.AllocationId).toEqual({ "Fn::GetAtt": [eipId, "AllocationId"] });
    expect(nat.Properties.SubnetId).toBeDefined();
    expect(natId).toBeDefined();
  });

  test("routes only application subnets through the single NAT and keeps database subnets isolated", () => {
    const template = networkTemplate(stage);
    const routes = Object.values(template.findResources("AWS::EC2::Route"));
    const applicationRouteTables = namedRouteTables(template, "application");
    const databaseRouteTables = namedRouteTables(template, "database");

    expect(applicationRouteTables).toHaveLength(2);
    expect(databaseRouteTables).toHaveLength(2);
    expect(routes.filter((route) =>
      applicationRouteTables.some((id) => JSON.stringify(route.Properties.RouteTableId).includes(id))
      && route.Properties.DestinationCidrBlock === "0.0.0.0/0"
      && route.Properties.NatGatewayId !== undefined,
    )).toHaveLength(2);
    expect(routes.filter((route) =>
      databaseRouteTables.some((id) => JSON.stringify(route.Properties.RouteTableId).includes(id))
      && route.Properties.DestinationCidrBlock === "0.0.0.0/0",
    )).toHaveLength(0);
  });

  test("limits the S3 gateway endpoint to application route tables and existing buckets", () => {
    const template = networkTemplate(stage);
    const endpoints = Object.values(template.findResources("AWS::EC2::VPCEndpoint"));
    const applicationRouteTables = namedRouteTables(template, "application");

    expect(endpoints).toHaveLength(1);
    const endpoint = endpoints[0].Properties;
    expect(endpoint.VpcEndpointType).toBe("Gateway");
    expect(endpoint.RouteTableIds).toHaveLength(2);
    expect(JSON.stringify(endpoint.RouteTableIds)).toContain(applicationRouteTables[0]);
    expect(JSON.stringify(endpoint.RouteTableIds)).toContain(applicationRouteTables[1]);
    expect(endpoint.PolicyDocument.Statement[0].Resource).toEqual(expect.arrayContaining([
      "arn:aws:s3:::aura-historia-binary-artifacts-eu-central-1/*",
      "arn:aws:s3:::aura-historia-mail-templates-eu-central-1/*",
      "arn:aws:s3:::aura-historia-cfn-artifcats-eu-central-1/*",
    ]));
  });

  test("allows PostgreSQL only from named application, DMS, and migration groups", () => {
    const template = networkTemplate(stage);
    const groups = Object.entries(template.findResources("AWS::EC2::SecurityGroup"));
    const database = groups.find(([, group]) => group.Properties.GroupDescription === "Private PostgreSQL database boundary");

    expect(groups).toHaveLength(5);
    expect(database).toBeDefined();
    // CDK emits an impossible ICMP rule to represent allowAllOutbound=false.
    expect(database![1].Properties.SecurityGroupEgress).toEqual([{
      CidrIp: "255.255.255.255/32",
      Description: "Disallow all traffic",
      FromPort: 252,
      IpProtocol: "icmp",
      ToPort: 86,
    }]);
    const databaseIngress = Object.values(template.findResources("AWS::EC2::SecurityGroupIngress"))
      .filter((rule) => JSON.stringify(rule.Properties.GroupId).includes(database![0]));
    expect(databaseIngress).toHaveLength(3);
    for (const rule of databaseIngress) {
      expect(rule.Properties.FromPort).toBe(5432);
      expect(rule.Properties.ToPort).toBe(5432);
      expect(rule.Properties.SourceSecurityGroupId).toBeDefined();
      expect(rule.Properties.CidrIp).toBeUndefined();
    }
  });

  test("allows DMS egress only to PostgreSQL and its private AWS API endpoint boundary", () => {
    const template = networkTemplate(stage);
    const groups = Object.entries(template.findResources("AWS::EC2::SecurityGroup"));
    const dms = groups.find(([, group]) => group.Properties.GroupDescription === "Private DMS replication instances");
    const endpoint = groups.find(([, group]) => group.Properties.GroupDescription === "DMS interface endpoint boundary");

    expect(dms).toBeDefined();
    expect(endpoint).toBeDefined();
    const dmsEgress = Object.values(template.findResources("AWS::EC2::SecurityGroupEgress"))
      .filter((rule) => JSON.stringify(rule.Properties.GroupId).includes(dms![0]));
    expect(dmsEgress).toHaveLength(2);
    expect(dmsEgress.map((rule) => rule.Properties.FromPort).sort()).toEqual([443, 5432]);
    expect(dmsEgress.every((rule) => rule.Properties.CidrIp === undefined)).toBe(true);

    const endpointIngress = Object.values(template.findResources("AWS::EC2::SecurityGroupIngress"))
      .filter((rule) => JSON.stringify(rule.Properties.GroupId).includes(endpoint![0]));
    expect(endpointIngress).toHaveLength(3);
    expect(endpointIngress).toEqual(expect.arrayContaining([
      expect.objectContaining({
        Properties: expect.objectContaining({
          Description: "DMS AWS API calls",
          FromPort: 443,
          ToPort: 443,
          SourceSecurityGroupId: expect.anything(),
        }),
      }),
      expect.objectContaining({
        Properties: expect.objectContaining({
          Description: "Application runtime Secrets Manager calls",
          FromPort: 443,
          ToPort: 443,
          SourceSecurityGroupId: expect.anything(),
        }),
      }),
      expect.objectContaining({
        Properties: expect.objectContaining({
          Description: "Migration runtime Secrets Manager calls",
          FromPort: 443,
          ToPort: 443,
          SourceSecurityGroupId: expect.anything(),
        }),
      }),
    ]));
  });

  test("attaches only PostgreSQL Lambdas to private application subnets", () => {
    const app = new cdk.App({ analyticsReporting: false });
    const stacks = createApplicationStacks(app, { stage });
    const compute = Template.fromStack(stacks.compute);
    const initialization = Template.fromStack(stacks.initialization!);
    const computeFunctions = Object.values(compute.findResources("AWS::Lambda::Function"));
    const initializationFunctions = Object.values(initialization.findResources("AWS::Lambda::Function"));
    const applicationFunctions = [...computeFunctions, ...initializationFunctions].filter((resource) =>
      [
        "aura-historia-api",
        "cognito-post-confirmation",
        "shopify-lambda",
        "stripe-lambda",
        "fxrate-lambda",
        "product-listing-opensearch-lambda",
        "search-filter-projection-lambda",
        "product-listing-normalization-lambda",
        "product-content-assessment-lambda",
        "product-embedding-lambda",
        "search-filter-percolator-lambda",
        "search-filter-match-notification-lambda",
        "watchlist-notification-lambda",
        "notification-delivery-lambda",
      ]
        .some((name) => resource.Properties.FunctionName === `${name}-${stage}`),
    );
    const migration = initializationFunctions.find((resource) =>
      resource.Properties.FunctionName === `database-migration-lambda-${stage}`,
    );
    const logRetention = computeFunctions.find((resource) => resource.Properties.FunctionName === `cloudwatch-log-retention-lambda-${stage}`);
    const groups = Object.entries(networkTemplate(stage).findResources("AWS::EC2::SecurityGroup"));
    const migrationSecurityGroup = groups
      .find(([, group]) => group.Properties.GroupDescription === "Approved database migration workload boundary");
    const migrationEgress = Object.values(networkTemplate(stage).findResources("AWS::EC2::SecurityGroupEgress"))
      .filter((rule) => JSON.stringify(rule.Properties.GroupId).includes(migrationSecurityGroup?.[0] ?? ""));

    const vpcAttachedFunctions = [...computeFunctions, ...initializationFunctions]
      .filter((resource) => resource.Properties.VpcConfig !== undefined);
    expect(applicationFunctions).toHaveLength(14);
    expect(applicationFunctions.every((resource) => resource.Properties.VpcConfig !== undefined)).toBe(true);
    expect(migrationSecurityGroup).toBeDefined();
    expect(migration?.Properties.VpcConfig).toBeDefined();
    expect(JSON.stringify(migration?.Properties.VpcConfig.SecurityGroupIds)).toContain(migrationSecurityGroup![0]);
    expect(migrationEgress).toHaveLength(2);
    expect(migrationEgress.map((rule) => rule.Properties.FromPort).sort()).toEqual([443, 5432]);
    expect(migrationEgress.every((rule) => rule.Properties.CidrIp === undefined)).toBe(true);
    expect(vpcAttachedFunctions).toHaveLength(15);
    expect(logRetention?.Properties.VpcConfig).toBeUndefined();
  });
});

test("ephemeral stacks do not create paid VPC, NAT, EIP, endpoint, or security group topology", () => {
  const app = new cdk.App({ analyticsReporting: false });
  const stacks = createApplicationStacks(app, { stage: "ephemeral" });

  expect(stacks.network).toBeUndefined();
  const compute = Template.fromStack(stacks.compute);
  compute.resourceCountIs("AWS::EC2::NatGateway", 0);
  compute.resourceCountIs("AWS::EC2::EIP", 0);
  compute.resourceCountIs("AWS::EC2::VPCEndpoint", 0);
  compute.resourceCountIs("AWS::EC2::SecurityGroup", 0);
});
