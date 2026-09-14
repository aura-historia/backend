#!/usr/bin/env node
import * as cdk from "aws-cdk-lib";
import { ApplicationEphemeralStack, createApplicationStacks } from "../src/application-stack";
import { isStageName } from "../src/config";
import { loadPostgresLambdaConfig } from "../src/postgres-lambda-config";

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
const singleStack = app.node.tryGetContext("singleStack") === "true" || process.env.SINGLE_STACK === "true";
// Explicit local operator input only. Legacy workflows do not opt in by accident.
const postgresLambdaFile = app.node.tryGetContext("postgresLambdaConfig");
const postgresLambda = postgresLambdaFile === undefined ? undefined
  : loadPostgresLambdaConfig(postgresLambdaFile, stageContext);

if (singleStack) {
  if (stageContext !== "ephemeral") {
    throw new Error("singleStack mode is only supported for the ephemeral stage.");
  }

  new ApplicationEphemeralStack(app, stackNamePrefix, {
    stage: stageContext,
    stackName: stackNamePrefix,
    localStackMappedPort,
  });
} else {
  createApplicationStacks(app, {
    stage: stageContext,
    postgresLambda,
    env: postgresLambda?.environment,
    stackNamePrefix,
    localStackMappedPort,
  });
}
