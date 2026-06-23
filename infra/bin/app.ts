#!/usr/bin/env node
import * as cdk from "aws-cdk-lib";
import { ApplicationStack } from "../src/application-stack";
import { isStageName } from "../src/config";

const app = new cdk.App({
  analyticsReporting: false,
});

const stageContext = app.node.tryGetContext("stage") ?? process.env.STAGE ?? "dev";
if (!isStageName(stageContext)) {
  throw new Error(`Unsupported stage '${stageContext}'. Expected one of: prod, dev, ephemeral.`);
}

const defaultStackName = `application-${stageContext}`;
const stackName = app.node.tryGetContext("stackName") ?? process.env.STACK_NAME ?? defaultStackName;

new ApplicationStack(app, stackName, {
  stage: stageContext,
  stackName,
});
