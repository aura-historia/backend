import * as cdk from "aws-cdk-lib";
import { Template } from "aws-cdk-lib/assertions";
import { createApplicationStacks } from "../src/application-stack";
import { STAGES } from "../src/config";

describe.each(STAGES)("%s effective listing shipping", stage => {
  test("wires bounded database fanout with compact CDC and no provider permissions", () => {
    const stacks = createApplicationStacks(new cdk.App({ analyticsReporting: false }), { stage });
    const compute = Template.fromStack(stacks.compute);
    compute.hasResourceProperties("AWS::Lambda::Function", {
      FunctionName: `listing-shipping-lambda-${stage}`,
      Timeout: 45,
      MemorySize: 256,
      ReservedConcurrentExecutions: 2,
      Environment: { Variables: { POSTGRES_MAX_CONNECTIONS: "1" } },
    });
    const mappings = Object.values(compute.findResources("AWS::Lambda::EventSourceMapping"));
    const mapping = mappings.find(m =>
      JSON.stringify(m.Properties.FunctionName).includes("ListingShippingVersion"),
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
      JSON.stringify(p.Properties.Roles).includes("ListingShippingLambdaServiceRole"),
    );
    expect(policy).toBeDefined();
    expect(JSON.stringify(policy)).not.toMatch(/geo-places:|sqs:SendMessage|es:ESHttp|ses:SendEmail/);
    expect(JSON.stringify(policies)).not.toContain("product-listings/_refresh");
    const projectorPolicy = policies.find(p =>
      JSON.stringify(p.Properties.Roles).includes("ProductListingOpenSearchLambdaServiceRole"),
    );
    expect(projectorPolicy).toBeDefined();
    const searchStatements = projectorPolicy!.Properties.PolicyDocument.Statement
      .filter((statement: { Action: string | string[] }) =>
        [statement.Action].flat().some(action => action.startsWith("es:")),
      );
    expect(searchStatements).toHaveLength(1);
    expect([searchStatements[0].Action].flat()).toEqual(["es:ESHttpPost"]);
    expect(JSON.stringify(searchStatements[0].Resource)).toContain("/product-listings/_doc/*");

    const data = Template.fromStack(stacks.data);
    const task = Object.values(data.findResources("AWS::DMS::ReplicationTask"))[0];
    const tables = JSON.parse(task.Properties.TableMappings).rules
      .filter((r: Record<string, string>) => r["rule-type"] === "selection")
      .map((r: Record<string, Record<string, string>>) => r["object-locator"]["table-name"]);
    expect(tables).toContain("listing_shipping_wakeups");
    for (const table of [
      "party_locations", "shipping_profiles", "shipping_changes", "shipping_profile_evidence", "shipping_source_defaults", "geo_resolutions",
      "product_listing_shipping", "product_listing_shipping_dependencies", "listing_shipping_work",
    ]) {
      expect(tables).not.toContain(table);
    }
  });
});
