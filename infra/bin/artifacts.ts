#!/usr/bin/env node
import * as cdk from "aws-cdk-lib";
import { ContainerArtifactStack } from "../src/artifact-stack";
import { WORKLOAD_REGION } from "../src/config";

const app = new cdk.App({ analyticsReporting: false });
const account = app.node.tryGetContext("account") ?? process.env.CDK_DEFAULT_ACCOUNT;
const region = app.node.tryGetContext("region") ?? process.env.CDK_DEFAULT_REGION ?? WORKLOAD_REGION;
if (account !== undefined && !/^\d{12}$/.test(account)) throw new Error("The account must be a 12-digit AWS account ID.");
if (region !== WORKLOAD_REGION) throw new Error(`Artifact region must be ${WORKLOAD_REGION}.`);
new ContainerArtifactStack(app, "aura-historia-periodic-matcher-artifacts", {
  env: { account, region },
  synthesizer: new cdk.CliCredentialsStackSynthesizer(),
});
