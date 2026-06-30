#!/usr/bin/env node
import * as cdk from "aws-cdk-lib";
import { createApplicationStacks } from "../src/application-stack";
import { isStageName } from "../src/config";

const app = new cdk.App({
  analyticsReporting: false,
});

const stageContext = app.node.tryGetContext("stage") ?? process.env.STAGE ?? "dev";
if (!isStageName(stageContext)) {
  throw new Error(`Unsupported stage '${stageContext}'. Expected one of: prod, dev, ephemeral.`);
}

const defaultStackNamePrefix = `application-${stageContext}`;
const stackNamePrefix = app.node.tryGetContext("stackNamePrefix") ?? process.env.STACK_NAME_PREFIX ?? app.node.tryGetContext("stackName") ?? process.env.STACK_NAME ?? defaultStackNamePrefix;
const localStackMappedPort = app.node.tryGetContext("localStackMappedPort") ?? process.env.LOCALSTACK_MAPPED_PORT;

createApplicationStacks(app, {
  stage: stageContext,
  stackNamePrefix,
  localStackMappedPort,
});
