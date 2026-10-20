#!/usr/bin/env node
import * as cdk from "aws-cdk-lib";
import { ReleaseOperationsStack } from "../src/release-operations-stack";
import { CLOUDFORMATION_STAGING_BUCKET_NAME, WORKLOAD_REGION } from "../src/config";

const app = new cdk.App({ analyticsReporting: false });
const account = app.node.tryGetContext("account") ?? process.env.CDK_DEFAULT_ACCOUNT;
if (account !== undefined && !/^\d{12}$/.test(account))
  throw new Error("The account must be a 12-digit AWS account ID.");
new ReleaseOperationsStack(app, "aura-historia-release-operations", {
  env: { account, region: WORKLOAD_REGION },
  synthesizer: new cdk.CliCredentialsStackSynthesizer({ fileAssetsBucketName: CLOUDFORMATION_STAGING_BUCKET_NAME }),
});
