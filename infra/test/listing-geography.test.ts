import * as cdk from "aws-cdk-lib";
import { Template } from "aws-cdk-lib/assertions";
import { createApplicationStacks } from "../src/application-stack";
import { STAGES } from "../src/config";

describe.each(STAGES)("%s effective listing geography", stage => {
  test("wires bounded database fanout with compact CDC and no provider permissions", () => {
    const stacks = createApplicationStacks(new cdk.App({ analyticsReporting: false }), { stage });
    const compute = Template.fromStack(stacks.compute);
    compute.hasResourceProperties("AWS::Lambda::Function", {
      FunctionName: `listing-geography-lambda-${stage}`,
      Timeout: 45,
      MemorySize: 256,
      ReservedConcurrentExecutions: 2,
      Environment: { Variables: { POSTGRES_MAX_CONNECTIONS: "1" } },
    });
    const mappings = Object.values(compute.findResources("AWS::Lambda::EventSourceMapping"));
    const mapping = mappings.find(m =>
      JSON.stringify(m.Properties.FunctionName).includes("ListingGeographyVersion"),
    );
    expect(mapping?.Properties).toMatchObject({
      Enabled: true,
      BatchSize: 1,
      ScalingConfig: { MaximumConcurrency: 2 },
      FunctionResponseTypes: ["ReportBatchItemFailures"],
    });
    expect(mapping?.DependsOn).toBeDefined();

    const policies = Object.values(compute.findResources("AWS::IAM::Policy"));
    const policy = policies.find(p =>
      JSON.stringify(p.Properties.Roles).includes("ListingGeographyLambdaServiceRole"),
    );
    expect(policy).toBeDefined();
    expect(JSON.stringify(policy)).not.toMatch(/geo-places:|sqs:SendMessage|es:ESHttp|ses:SendEmail/);
    const refreshPolicies = policies.filter(p =>
      JSON.stringify(p.Properties.PolicyDocument).includes("product-listings/_refresh"),
    );
    expect(refreshPolicies).toHaveLength(1);
    expect(JSON.stringify(refreshPolicies[0].Properties.Roles)).toContain("ProductListingOpenSearchLambdaServiceRole");
    const refresh = refreshPolicies[0].Properties.PolicyDocument.Statement.find(
      (statement: { Action: string }) => statement.Action === "es:ESHttpPost",
    );
    expect(JSON.stringify(refresh.Resource)).toContain("product-listings/_refresh");

    const data = Template.fromStack(stacks.data);
    const task = Object.values(data.findResources("AWS::DMS::ReplicationTask"))[0];
    const tables = JSON.parse(task.Properties.TableMappings).rules
      .filter((r: Record<string, string>) => r["rule-type"] === "selection")
      .map((r: Record<string, Record<string, string>>) => r["object-locator"]["table-name"]);
    expect(tables).toContain("listing_geography_wakeups");
    for (const table of [
      "party_locations", "listing_source_location_defaults", "geo_resolutions",
      "product_listing_geographies", "listing_geography_work", "listing_geography_gate",
    ]) {
      expect(tables).not.toContain(table);
    }
  });
});
