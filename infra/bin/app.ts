#!/usr/bin/env node
import * as cdk from "aws-cdk-lib";
import { createApplicationStacks } from "../src/application-stack";
import { isStageName, WORKLOAD_REGION } from "../src/config";

const app = new cdk.App({
  analyticsReporting: false,
});

const stageContext = app.node.tryGetContext("stage") ?? process.env.STAGE ?? "dev";
if (!isStageName(stageContext)) {
  throw new Error(`Unsupported stage '${stageContext}'. Expected one of: prod, dev.`);
}

const defaultStackNamePrefix = `application-${stageContext}`;
const stackNamePrefix = app.node.tryGetContext("stackNamePrefix") ?? process.env.STACK_NAME_PREFIX ?? app.node.tryGetContext("stackName") ?? process.env.STACK_NAME ?? defaultStackNamePrefix;
const deploymentAccount = app.node.tryGetContext("account") ?? process.env.CDK_DEFAULT_ACCOUNT;
// Template-only synth must not inherit an arbitrary CI runner region.
const deploymentRegion = app.node.tryGetContext("region")
  ?? (deploymentAccount === undefined ? WORKLOAD_REGION : process.env.CDK_DEFAULT_REGION ?? WORKLOAD_REGION);

if (deploymentAccount !== undefined && !/^\d{12}$/.test(deploymentAccount)) {
  throw new Error("The account context must be a 12-digit AWS account ID.");
}
if (deploymentRegion !== WORKLOAD_REGION) {
  throw new Error(`The backend workload region must be ${WORKLOAD_REGION}; received '${deploymentRegion}'.`);
}

const environment = {
  account: deploymentAccount,
  region: deploymentRegion,
};

createApplicationStacks(app, {
  ...environment,
  stage: stageContext,
  stackNamePrefix,
});
