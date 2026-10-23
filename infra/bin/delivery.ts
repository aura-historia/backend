#!/usr/bin/env node
import * as cdk from "aws-cdk-lib";
import { DeliveryStack } from "../src/delivery-stack";
import { WORKLOAD_REGION } from "../src/config";

const app = new cdk.App({ analyticsReporting: false });
const account = app.node.tryGetContext("account") ?? process.env.CDK_DEFAULT_ACCOUNT;
if (account !== undefined && !/^\d{12}$/.test(account)) throw new Error("Expected a 12-digit account.");
const devEnabled = app.node.tryGetContext("devEnabled") ?? "false";
if (!["true", "false"].includes(String(devEnabled))) throw new Error("devEnabled must be true or false.");
new DeliveryStack(app, "aura-historia-delivery", {
  env: { account, region: WORKLOAD_REGION },
  enabledTargets: devEnabled === "true" || devEnabled === true ? ["prod", "dev"] : ["prod"],
  connectionArn: app.node.tryGetContext("connectionArn"),
  controlKey: app.node.tryGetContext("controlKey") ?? "control/UNAUTHORIZED.zip",
  controlVersion: app.node.tryGetContext("controlVersion") ?? "UNAUTHORIZED",
  workflowRevision: app.node.tryGetContext("workflowRevision"),
  buildConfigDigest: app.node.tryGetContext("buildConfigDigest"),
  buildConfig: app.node.tryGetContext("buildConfig") ?? "ci/delivery/build-config.json",
  monthlyBudgetUsd: Number(app.node.tryGetContext("budgetUsd") ?? 50),
});
